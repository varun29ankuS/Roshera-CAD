//! Timeline-specific types shared across modules
//!
//! This module defines types specific to the timeline/event-sourcing system

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

/// Timeline-specific error type
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum TimelineError {
    /// Operation not found
    #[error("Operation not found: {id}")]
    NotFound { id: String },

    /// Invalid timeline operation
    #[error("Invalid timeline operation")]
    InvalidOperation,

    /// Timeline is locked
    #[error("Timeline is locked")]
    Locked,

    /// History corrupted
    #[error("Timeline history is corrupted")]
    HistoryCorrupted,

    /// Session expired
    #[error("Session expired: {id}")]
    Expired { id: String },

    /// Timeline operation failed
    #[error("Timeline operation failed: {operation}")]
    HistoryFailed { operation: String },

    /// Session not found
    #[error("Session not found in timeline")]
    SessionNotFound,

    /// No more operations to undo
    #[error("No more operations to undo")]
    NoMoreUndo,

    /// No more operations to redo
    #[error("No more operations to redo")]
    NoMoreRedo,

    /// Branch not found
    #[error("Branch not found: {0}")]
    BranchNotFound(BranchId),

    /// Event not found
    #[error("Event not found: {0}")]
    EventNotFound(EventId),

    /// Operation not implemented
    #[error("Operation not implemented: {0}")]
    NotImplemented(String),

    /// Execution error
    #[error("Execution error: {0}")]
    ExecutionError(String),
}

/// Unique identifier for timeline events
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EventId(pub Uuid);

impl EventId {
    /// Create a new event ID
    pub fn new() -> Self {
        EventId(Uuid::new_v4())
    }

    /// Create from existing UUID
    pub fn from_uuid(id: Uuid) -> Self {
        EventId(id)
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

/// Unique identifier for timeline branches
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BranchId(pub Uuid);

impl BranchId {
    /// Create a new branch ID
    pub fn new() -> Self {
        BranchId(Uuid::new_v4())
    }

    /// Get the main branch ID
    pub fn main() -> Self {
        // Use a fixed UUID for the main branch
        BranchId(Uuid::from_bytes([0; 16]))
    }

    /// Check if this is the main branch
    pub fn is_main(&self) -> bool {
        self.0 == Uuid::from_bytes([0; 16])
    }
}

impl Default for BranchId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for BranchId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_main() {
            write!(f, "main")
        } else {
            write!(f, "{}", self.0)
        }
    }
}

/// Timeline event representing a single operation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimelineEvent {
    /// Unique event identifier
    pub id: EventId,

    /// Operation that was performed
    pub operation: Operation,

    /// Operation outputs (created/modified/deleted entities)
    pub outputs: OperationOutputs,

    /// Timestamp when the event occurred
    pub timestamp: chrono::DateTime<chrono::Utc>,

    /// Session that created this event
    pub session_id: Uuid,

    /// Branch this event belongs to
    pub branch_id: BranchId,
}

/// Operation types that can be recorded in the timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Operation {
    /// Create a primitive shape
    CreatePrimitive {
        shape_type: crate::PrimitiveType,
        parameters: crate::ShapeParameters,
        position: crate::Position3D,
    },

    /// Boolean operation
    Boolean {
        operation: crate::BooleanOp,
        objects: Vec<crate::GeometryId>,
    },

    /// Transform operation
    Transform {
        object_id: crate::GeometryId,
        transform: crate::Transform3D,
    },

    /// Extrude operation
    Extrude {
        sketch_id: crate::GeometryId,
        distance: f64,
        direction: Option<crate::Vector3D>,
    },

    /// Delete operation
    Delete { object_ids: Vec<crate::GeometryId> },

    /// Other operations can be added here
    Custom {
        name: String,
        parameters: serde_json::Value,
    },
}

/// Outputs from an operation
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OperationOutputs {
    /// Entities created by this operation
    pub created: Vec<EntityReference>,

    /// Entities modified by this operation
    pub modified: Vec<EntityReference>,

    /// Entities deleted by this operation
    pub deleted: Vec<EntityReference>,
}

/// Reference to an entity in the timeline
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityReference {
    /// Entity ID
    pub id: crate::GeometryId,

    /// Entity type
    pub entity_type: EntityType,
}

/// Types of entities in the timeline
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntityType {
    /// Solid body
    Solid,
    /// Surface
    Surface,
    /// Curve
    Curve,
    /// Point
    Point,
    /// Sketch
    Sketch,
    /// Face
    Face,
    /// Edge
    Edge,
    /// Vertex
    Vertex,
}
