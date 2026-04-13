//! Core type definitions for the Timeline Engine

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

/// Unique identifier for timeline events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(pub Uuid);

impl EventId {
    /// Create a new EventId
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for EventId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for EventId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for branches
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BranchId(pub Uuid);

impl BranchId {
    /// Create a new BranchId
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    /// Main branch ID (always zeros)
    pub fn main() -> Self {
        Self(Uuid::nil())
    }

    /// Check if this is the main branch
    pub fn is_main(&self) -> bool {
        self.0.is_nil()
    }
}

impl Default for BranchId {
    fn default() -> Self {
        Self::main()
    }
}

impl std::fmt::Display for BranchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for entities (geometry objects)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityId(pub Uuid);

impl EntityId {
    /// Create a new EntityId
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Session identifier
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    /// Create a new SessionId
    pub fn new(id: String) -> Self {
        Self(id)
    }
}

/// Checkpoint identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CheckpointId(pub Uuid);

impl CheckpointId {
    /// Create a new CheckpointId
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl std::fmt::Display for CheckpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Snapshot identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SnapshotId(pub Uuid);

impl SnapshotId {
    /// Create a new SnapshotId
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

/// Sequential event index in timeline
pub type EventIndex = u64;

/// Main timeline event structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    /// Unique identifier
    pub id: EventId,

    /// Sequential position in timeline
    pub sequence_number: EventIndex,

    /// When the event was created
    pub timestamp: DateTime<Utc>,

    /// Who created this event
    pub author: Author,

    /// The operation performed
    pub operation: Operation,

    /// What this operation needs
    pub inputs: OperationInputs,

    /// What this operation produces
    pub outputs: OperationOutputs,

    /// Additional metadata
    pub metadata: EventMetadata,
}

/// Special checkpoint events for grouping operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    /// Checkpoint identifier
    pub id: CheckpointId,

    /// Name of the checkpoint
    pub name: String,

    /// Description of what was achieved
    pub description: String,

    /// Events included in this checkpoint
    pub event_range: (EventIndex, EventIndex),

    /// Author who created the checkpoint
    pub author: Author,

    /// When the checkpoint was created
    pub timestamp: DateTime<Utc>,

    /// Tags for categorization
    pub tags: Vec<String>,
}

/// Author of an event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Author {
    /// Human user
    User { id: String, name: String },
    /// AI agent
    AIAgent { id: String, model: String },
    /// System-generated
    System,
}

/// Event metadata
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventMetadata {
    /// Optional description
    pub description: Option<String>,

    /// Branch this event belongs to
    pub branch_id: BranchId,

    /// Tags for categorization
    pub tags: Vec<String>,

    /// Custom properties
    pub properties: HashMap<String, serde_json::Value>,
}

/// Geometry operations that can be performed
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Operation {
    // Creation operations
    /// Create a 2D sketch
    CreateSketch {
        /// Plane to sketch on
        plane: SketchPlane,
        /// Elements in the sketch
        elements: Vec<SketchElement>,
    },

    /// Create a 3D primitive
    CreatePrimitive {
        /// Type of primitive
        primitive_type: PrimitiveType,
        /// Parameters for the primitive
        parameters: serde_json::Value,
    },

    // Modification operations
    /// Extrude a sketch
    Extrude {
        /// Sketch to extrude
        sketch_id: EntityId,
        /// Extrusion distance
        distance: f64,
        /// Optional direction (default is normal to sketch plane)
        direction: Option<[f64; 3]>,
    },

    /// Revolve a sketch
    Revolve {
        /// Sketch to revolve
        sketch_id: EntityId,
        /// Axis of revolution
        axis: Axis,
        /// Angle in degrees
        angle: f64,
    },

    /// Loft between profiles
    Loft {
        /// Profile sketches
        profiles: Vec<EntityId>,
        /// Optional guide curves
        guide_curves: Option<Vec<EntityId>>,
    },

    /// Sweep along path
    Sweep {
        /// Profile to sweep
        profile: EntityId,
        /// Path to sweep along
        path: EntityId,
    },

    // Boolean operations
    /// Boolean union
    BooleanUnion {
        /// Objects to unite
        operands: Vec<EntityId>,
    },

    /// Boolean intersection
    BooleanIntersection {
        /// Objects to intersect
        operands: Vec<EntityId>,
    },

    /// Boolean difference
    BooleanDifference {
        /// Target object
        target: EntityId,
        /// Objects to subtract
        tools: Vec<EntityId>,
    },

    // Feature operations
    /// Add fillet
    Fillet {
        /// Edges to fillet
        edges: Vec<EntityId>,
        /// Fillet radius
        radius: f64,
    },

    /// Add chamfer
    Chamfer {
        /// Edges to chamfer
        edges: Vec<EntityId>,
        /// Chamfer distance
        distance: f64,
        /// Optional angle
        angle: Option<f64>,
    },

    /// Create pattern
    Pattern {
        /// Features to pattern
        features: Vec<EntityId>,
        /// Pattern type and parameters
        pattern_type: PatternType,
    },

    // Modification operations
    /// Transform entities
    Transform {
        /// Entities to transform
        entities: Vec<EntityId>,
        /// Transformation matrix
        transformation: [[f64; 4]; 4],
    },

    /// Delete entities
    Delete {
        /// Entities to delete
        entities: Vec<EntityId>,
    },

    /// Modify entity properties
    Modify {
        /// Entity to modify
        entity: EntityId,
        /// Modifications to apply
        modifications: Vec<Modification>,
    },

    // Checkpoint operation
    /// Create a checkpoint
    CreateCheckpoint {
        /// Checkpoint name
        name: String,
        /// Description
        description: String,
        /// Tags
        tags: Vec<String>,
    },

    // Batch operations
    /// Batch of operations (used for squashing)
    Batch {
        /// Operations in the batch
        operations: Vec<Operation>,
        /// Description of the batch
        description: String,
    },

    // Generic operations (for commands not yet mapped)
    /// Generic boolean operation (legacy support)
    Boolean {
        /// Boolean operation type
        operation: BooleanType,
        /// First operand
        operand_a: EntityId,
        /// Second operand
        operand_b: EntityId,
    },

    /// Generic operation for unmapped commands
    Generic {
        /// Command type as string
        command_type: String,
        /// Parameters as JSON
        parameters: serde_json::Value,
    },
}

/// Operation input requirements
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperationInputs {
    /// Entities that must exist for this operation
    pub required_entities: Vec<EntityReference>,

    /// Optional entities (may influence result)
    pub optional_entities: Vec<EntityReference>,

    /// Operation-specific parameters
    pub parameters: serde_json::Value,
}

/// Reference to an entity with validation info
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityReference {
    /// Entity ID
    pub id: EntityId,
    /// Expected entity type
    pub expected_type: EntityType,
    /// Validation requirements
    pub validation: ValidationRequirement,
}

/// What an operation produces
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OperationOutputs {
    /// Main entities created
    pub created: Vec<CreatedEntity>,

    /// Entities that were modified
    pub modified: Vec<EntityId>,

    /// Entities that were deleted
    pub deleted: Vec<EntityId>,

    /// Side effects
    pub side_effects: Vec<SideEffect>,
}

/// Created entity information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreatedEntity {
    /// Entity ID
    pub id: EntityId,
    /// Entity type
    pub entity_type: EntityType,
    /// Optional name
    pub name: Option<String>,
}

/// Side effect of an operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideEffect {
    /// Type of side effect
    pub effect_type: String,
    /// Description
    pub description: String,
    /// Related entities
    pub entities: Vec<EntityId>,
}

/// Modified entity information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModifiedEntity {
    /// Entity ID
    pub id: EntityId,
    /// Entity type
    pub entity_type: EntityType,
    /// What was modified
    pub modifications: Vec<Modification>,
}

/// Deleted entity information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeletedEntity {
    /// Entity ID that was deleted
    pub id: EntityId,
    /// Entity type
    pub entity_type: EntityType,
    /// Whether deletion cascaded to dependent entities
    pub cascaded: bool,
    /// IDs of entities that were also deleted due to cascade
    pub cascaded_entities: Vec<EntityId>,
}

/// Type of modification made to an entity
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ModificationType {
    /// Geometry was transformed
    Transform,
    /// Material was changed
    Material,
    /// Visibility was changed
    Visibility,
    /// Properties were updated
    Properties,
    /// Topology was modified
    Topology,
    /// Custom modification
    Custom(String),
}

/// Types of entities
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EntityType {
    /// 2D sketch
    Sketch,
    /// 3D solid
    Solid,
    /// Surface
    Surface,
    /// Curve
    Curve,
    /// Point
    Point,
    /// Edge
    Edge,
    /// Face
    Face,
    /// Vertex
    Vertex,
}

/// Validation requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValidationRequirement {
    /// Must exist
    MustExist,
    /// Must be a specific type
    MustBeType(EntityType),
    /// Must satisfy a predicate
    MustSatisfy(String),
}

/// Sketch plane definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SketchPlane {
    /// XY plane
    XY,
    /// XZ plane
    XZ,
    /// YZ plane
    YZ,
    /// Custom plane
    Custom {
        /// Origin point
        origin: [f64; 3],
        /// Normal vector
        normal: [f64; 3],
        /// X direction
        x_dir: [f64; 3],
    },
}

/// Sketch elements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SketchElement {
    /// Line segment
    Line {
        /// Start point
        start: [f64; 2],
        /// End point
        end: [f64; 2],
    },
    /// Arc
    Arc {
        /// Center point
        center: [f64; 2],
        /// Radius
        radius: f64,
        /// Start angle (degrees)
        start_angle: f64,
        /// End angle (degrees)
        end_angle: f64,
    },
    /// Circle
    Circle {
        /// Center point
        center: [f64; 2],
        /// Radius
        radius: f64,
    },
    /// Rectangle
    Rectangle {
        /// Corner point
        corner: [f64; 2],
        /// Width
        width: f64,
        /// Height
        height: f64,
    },
}

/// Primitive types
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PrimitiveType {
    /// Box/Cuboid
    Box,
    /// Sphere
    Sphere,
    /// Cylinder
    Cylinder,
    /// Cone
    Cone,
    /// Torus
    Torus,
}

/// Boolean operation types
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum BooleanType {
    /// Union operation
    Union,
    /// Intersection operation
    Intersection,
    /// Difference operation
    Difference,
}

/// Axis definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Axis {
    /// Origin point
    pub origin: [f64; 3],
    /// Direction vector
    pub direction: [f64; 3],
}

/// Pattern types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PatternType {
    /// Linear pattern
    Linear {
        /// Direction
        direction: [f64; 3],
        /// Spacing
        spacing: f64,
        /// Count
        count: u32,
    },
    /// Circular pattern
    Circular {
        /// Axis
        axis: Axis,
        /// Count
        count: u32,
        /// Total angle (degrees)
        angle: f64,
    },
    /// Rectangular pattern
    Rectangular {
        /// X direction
        x_direction: [f64; 3],
        /// Y direction
        y_direction: [f64; 3],
        /// X spacing
        x_spacing: f64,
        /// Y spacing
        y_spacing: f64,
        /// X count
        x_count: u32,
        /// Y count
        y_count: u32,
    },
}

/// Modification types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Modification {
    /// Change name
    SetName(String),
    /// Change color
    SetColor([f32; 4]),
    /// Change material
    SetMaterial(String),
    /// Change visibility
    SetVisible(bool),
    /// Custom property
    SetProperty(String, serde_json::Value),
}

/// Types of dependencies
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DependencyType {
    /// Operation cannot proceed without this data
    DataRequirement {
        /// Whether a substitute can be used
        can_substitute: bool,
    },

    /// Operation references but could adapt
    Reference {
        /// Type of constraint
        constraint_type: ConstraintType,
    },

    /// Must happen after, but no data dependency
    Temporal,

    /// Dimensional relationship
    Dimensional {
        /// Parameter name
        parameter: String,
    },
}

/// Constraint types
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ConstraintType {
    /// Geometric constraint
    Geometric,
    /// Dimensional constraint
    Dimensional,
    /// Topological constraint
    Topological,
}

/// A branch in the timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Branch {
    /// Unique identifier
    pub id: BranchId,

    /// Human-readable name
    pub name: String,

    /// Where this branch diverged from
    pub fork_point: ForkPoint,

    /// Parent branch (if any)
    pub parent: Option<BranchId>,

    /// Events specific to this branch
    #[serde(skip)]
    pub events: Arc<DashMap<EventIndex, TimelineEvent>>,

    /// Current state of the branch
    pub state: BranchState,

    /// Metadata about the branch
    pub metadata: BranchMetadata,
}

/// Where a branch forked from
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkPoint {
    /// Branch ID it forked from
    pub branch_id: BranchId,
    /// Event index where it forked
    pub event_index: EventIndex,
    /// Timestamp of fork
    pub timestamp: DateTime<Utc>,
}

/// Current state of a branch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BranchState {
    /// Branch is active
    Active,
    /// Branch was merged
    Merged {
        /// Branch it was merged into
        into: BranchId,
        /// When it was merged
        at: DateTime<Utc>,
    },
    /// Branch was abandoned
    Abandoned {
        /// Reason for abandonment
        reason: String,
    },
    /// Branch is completed
    Completed {
        /// Quality score
        score: f64,
    },
}

/// Branch metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BranchMetadata {
    /// Who created the branch
    pub created_by: Author,
    /// When it was created
    pub created_at: DateTime<Utc>,
    /// Purpose of the branch
    pub purpose: BranchPurpose,
    /// AI context if applicable
    pub ai_context: Option<AIContext>,
    /// Checkpoints in this branch
    pub checkpoints: Vec<CheckpointId>,
}

/// Purpose of a branch
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BranchPurpose {
    /// User exploration
    UserExploration {
        /// Description
        description: String,
    },
    /// AI optimization
    AIOptimization {
        /// Optimization objective
        objective: OptimizationObjective,
    },
    /// What-if analysis
    WhatIfAnalysis {
        /// Parameters being varied
        parameters: Vec<String>,
    },
    /// Bug fix
    BugFix {
        /// Issue ID
        issue_id: String,
    },
    /// New feature
    Feature {
        /// Feature name
        feature_name: String,
    },
}

/// AI-specific branch context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AIContext {
    /// AI agent ID
    pub agent_id: String,
    /// Model being used
    pub model: String,
    /// Objective
    pub objective: String,
    /// Design constraints
    pub constraints: Vec<DesignConstraint>,
    /// Number of iterations
    pub iterations: u32,
    /// Current score
    pub current_score: f64,
}

/// Optimization objectives
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum OptimizationObjective {
    /// Minimize weight
    MinimizeWeight,
    /// Maximize strength
    MaximizeStrength,
    /// Minimize cost
    MinimizeCost,
    /// Minimize material usage
    MinimizeMaterial,
    /// Custom objective
    Custom(String),
}

/// Design constraints
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DesignConstraint {
    /// Constraint name
    pub name: String,
    /// Constraint type
    pub constraint_type: String,
    /// Parameters
    pub parameters: serde_json::Value,
}

/// Timeline engine configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineConfig {
    /// Storage configuration
    pub storage: StorageConfig,

    /// Cache configuration
    pub cache: CacheConfig,

    /// Execution configuration
    pub execution: ExecutionConfig,

    /// Checkpoint configuration
    pub checkpoints: CheckpointConfig,
}

impl Default for TimelineConfig {
    fn default() -> Self {
        Self {
            storage: StorageConfig::default(),
            cache: CacheConfig::default(),
            execution: ExecutionConfig::default(),
            checkpoints: CheckpointConfig::default(),
        }
    }
}

/// Storage configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Base path for storage
    pub base_path: PathBuf,
    /// Enable compression
    pub compression_enabled: bool,
    /// Snapshot interval (events)
    pub snapshot_interval: u32,
    /// Maximum event size
    pub max_event_size: usize,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            base_path: PathBuf::from("./timeline_data"),
            compression_enabled: true,
            snapshot_interval: 1000,
            max_event_size: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// Cache configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Maximum memory in MB
    pub max_memory_mb: usize,
    /// Time to live in seconds
    pub ttl_seconds: u64,
    /// Warm cache on startup
    pub warm_on_startup: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_memory_mb: 512,
            ttl_seconds: 3600, // 1 hour
            warm_on_startup: true,
        }
    }
}

/// Execution configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionConfig {
    /// Maximum parallel operations
    pub max_parallel_ops: usize,
    /// Operation timeout in seconds
    pub operation_timeout_secs: u64,
    /// Enable validation
    pub enable_validation: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_parallel_ops: 4,
            operation_timeout_secs: 30,
            enable_validation: true,
        }
    }
}

/// Checkpoint configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointConfig {
    /// Auto checkpoint interval
    pub auto_checkpoint_interval: Option<u32>,
    /// Maximum events between checkpoints
    pub max_events_between_checkpoints: u32,
    /// Create checkpoint on branch creation
    pub checkpoint_on_branch_create: bool,
}

impl Default for CheckpointConfig {
    fn default() -> Self {
        Self {
            auto_checkpoint_interval: Some(100),
            max_events_between_checkpoints: 500,
            checkpoint_on_branch_create: true,
        }
    }
}

/// Merge strategy for branches
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum MergeStrategy {
    /// Fast-forward if possible
    FastForward,
    /// Always create merge commit
    NoFastForward,
    /// Rebase source onto target
    Rebase,
    /// Squash all commits into one
    Squash,
}

/// Result of a merge operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeResult {
    /// Number of conflicts resolved
    pub conflicts_resolved: usize,
    /// Number of operations merged
    pub operations_merged: usize,
    /// New head event after merge
    pub new_head: EventId,
}
