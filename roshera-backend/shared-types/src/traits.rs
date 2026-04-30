//! Trait definitions for cross-module interfaces
//!
//! This module defines the core traits that establish contracts between
//! different modules in the Roshera CAD system. These traits enforce
//! type safety at the crate boundary so the geometry kernel, timeline,
//! session manager, and exporters can talk through stable interfaces.

use crate::{BooleanOp, GeometryId, MaterialProperties, Mesh, Transform3D};
#[cfg(not(target_arch = "wasm32"))]
use async_trait::async_trait;
use std::sync::Arc;
use uuid::Uuid;

/// Core trait for any geometric entity in the system
///
/// Every geometric type (vertex, edge, face, solid, sketch element)
/// implements this trait so that callers can reason about identity,
/// bounds, and bookkeeping without depending on the concrete type.
pub trait GeometricEntity: Send + Sync {
    /// Get the unique identifier for this entity
    fn id(&self) -> GeometryId;

    /// Get the mesh representation for rendering/export
    fn tessellate(&self, tolerance: f32) -> Result<Mesh, crate::GeometryError>;

    /// Apply a transformation to this entity
    fn transform(&mut self, transform: &Transform3D) -> Result<(), crate::GeometryError>;

    /// Get the bounding box of this entity
    fn bounds(&self) -> crate::BoundingBox;

    /// Clone this entity with a new ID
    fn clone_with_new_id(&self) -> Box<dyn GeometricEntity>;

    /// Validate the entity's topology
    fn validate(&self) -> Result<(), crate::GeometryError>;
}

/// Trait for mesh-like objects that can be converted between different representations
///
/// This solves the ThreeJsMesh vs shared_types::Mesh issue
pub trait MeshConvertible {
    /// Convert to shared mesh representation
    fn to_shared_mesh(&self) -> Mesh;

    /// Create from shared mesh representation
    fn from_shared_mesh(mesh: &Mesh) -> Self;

    /// Get vertex count
    fn vertex_count(&self) -> usize;

    /// Get triangle count
    fn triangle_count(&self) -> usize;
}

/// Trait for material properties that can be converted between representations
///
/// This solves the multiple MaterialProperties definitions issue
pub trait MaterialConvertible {
    /// Convert to shared material representation
    fn to_shared_material(&self) -> MaterialProperties;

    /// Create from shared material representation
    fn from_shared_material(material: &MaterialProperties) -> Self;
}

/// Trait for ID types that can be converted between String and UUID
///
/// This solves the GeometryId(String) vs GeometryId(Uuid) issue
pub trait IdConvertible {
    /// Convert to UUID
    fn to_uuid(&self) -> Uuid;

    /// Convert to String
    fn to_string(&self) -> String;

    /// Create from UUID
    fn from_uuid(id: Uuid) -> Self;

    /// Create from String
    fn from_string(id: String) -> Result<Self, crate::GeometryError>
    where
        Self: Sized;
}

/// Trait for timeline operations that can be executed
///
/// This provides the missing ExecutionEngine methods
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
pub trait TimelineExecutable: Send + Sync {
    /// Record an operation in the timeline
    async fn record_operation(
        &self,
        operation: crate::Operation,
        session_id: Uuid,
    ) -> Result<crate::EventId, crate::TimelineError>;

    /// Undo the last operation for a session
    async fn undo(&self, session_id: Uuid) -> Result<crate::EventId, crate::TimelineError>;

    /// Redo the last undone operation for a session
    async fn redo(&self, session_id: Uuid) -> Result<crate::EventId, crate::TimelineError>;

    /// Get the operation history for a session
    async fn get_history(
        &self,
        session_id: Uuid,
        limit: Option<usize>,
    ) -> Result<Vec<crate::TimelineEvent>, crate::TimelineError>;

    /// Execute a specific event
    async fn execute_event(&self, event: &crate::TimelineEvent)
        -> Result<(), crate::TimelineError>;
}

/// Trait for geometry operations that can be performed
///
/// This establishes the contract for the geometry engine
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
pub trait GeometryOperations: Send + Sync {
    /// Perform a boolean operation
    async fn boolean_operation(
        &self,
        op: BooleanOp,
        objects: Vec<GeometryId>,
    ) -> Result<GeometryId, crate::GeometryError>;

    /// Create a primitive shape
    async fn create_primitive(
        &self,
        shape_type: crate::PrimitiveType,
        parameters: crate::ShapeParameters,
    ) -> Result<GeometryId, crate::GeometryError>;

    /// Extrude a face or sketch
    async fn extrude(
        &self,
        profile_id: GeometryId,
        distance: f64,
        direction: Option<[f32; 3]>,
    ) -> Result<GeometryId, crate::GeometryError>;

    /// Revolve a profile around an axis
    async fn revolve(
        &self,
        profile_id: GeometryId,
        axis: [f32; 3],
        angle: f64,
    ) -> Result<GeometryId, crate::GeometryError>;

    /// Apply fillet to edges
    async fn fillet(
        &self,
        edges: Vec<String>,
        radius: f64,
    ) -> Result<GeometryId, crate::GeometryError>;

    /// Apply chamfer to edges
    async fn chamfer(
        &self,
        edges: Vec<String>,
        distance: f64,
    ) -> Result<GeometryId, crate::GeometryError>;
}

/// Trait for session management operations
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
pub trait SessionOperations: Send + Sync {
    /// Create a new session
    async fn create_session(&self, name: String) -> String;

    /// Join an existing session
    async fn join_session(&self, session_id: &str) -> Result<(), crate::SessionError>;

    /// Leave a session
    async fn leave_session(&self, session_id: &str) -> Result<(), crate::SessionError>;

    /// Add an object to a session
    async fn add_object(
        &self,
        session_id: &str,
        object: crate::CADObject,
    ) -> Result<(), crate::SessionError>;

    /// Remove an object from a session
    async fn remove_object(
        &self,
        session_id: &str,
        object_id: &str,
    ) -> Result<(), crate::SessionError>;

    /// Get session state
    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Arc<std::sync::RwLock<crate::SessionState>>, crate::SessionError>;
}

/// Trait for export operations
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
pub trait ExportOperations: Send + Sync {
    /// Export to STL format
    async fn export_stl(
        &self,
        objects: Vec<GeometryId>,
        binary: bool,
    ) -> Result<Vec<u8>, crate::ExportError>;

    /// Export to OBJ format
    async fn export_obj(
        &self,
        objects: Vec<GeometryId>,
        include_materials: bool,
    ) -> Result<Vec<u8>, crate::ExportError>;

    /// Export to STEP format
    async fn export_step(&self, objects: Vec<GeometryId>) -> Result<Vec<u8>, crate::ExportError>;

    /// Export to IGES format
    async fn export_iges(&self, objects: Vec<GeometryId>) -> Result<Vec<u8>, crate::ExportError>;
}

/// Trait for AI command processing
#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
pub trait AICommandProcessor: Send + Sync {
    /// Process a text command
    async fn process_text_command(
        &self,
        text: &str,
        context: Option<serde_json::Value>,
    ) -> Result<Vec<crate::AICommand>, crate::CommandError>;

    /// Process voice input
    async fn process_voice_command(
        &self,
        audio_data: &[u8],
        format: &str,
    ) -> Result<Vec<crate::AICommand>, crate::CommandError>;

    /// Generate design suggestions
    async fn suggest_next_operation(
        &self,
        current_state: &serde_json::Value,
    ) -> Result<Vec<String>, crate::CommandError>;
}

/// Implementation of IdConvertible for GeometryId
impl IdConvertible for GeometryId {
    fn to_uuid(&self) -> Uuid {
        // GeometryId now contains a UUID directly
        self.0
    }

    fn to_string(&self) -> String {
        self.0.to_string()
    }

    fn from_uuid(id: Uuid) -> Self {
        GeometryId(id)
    }

    fn from_string(id: String) -> Result<Self, crate::GeometryError> {
        match Uuid::parse_str(&id) {
            Ok(uuid) => Ok(GeometryId(uuid)),
            Err(_) => Err(crate::GeometryError::InvalidGeometryId {
                id,
                reason: "Invalid UUID format".to_string(),
            }),
        }
    }
}

impl From<String> for GeometryId {
    fn from(s: String) -> Self {
        match Uuid::parse_str(&s) {
            Ok(uuid) => GeometryId(uuid),
            Err(_) => {
                // Generate deterministic UUID from string using v5 namespace
                GeometryId(Uuid::new_v5(&Uuid::NAMESPACE_OID, s.as_bytes()))
            }
        }
    }
}

impl From<&str> for GeometryId {
    fn from(s: &str) -> Self {
        match Uuid::parse_str(s) {
            Ok(uuid) => GeometryId(uuid),
            Err(_) => {
                // Generate deterministic UUID from string using v5 namespace
                GeometryId(Uuid::new_v5(&Uuid::NAMESPACE_OID, s.as_bytes()))
            }
        }
    }
}

/// Performance monitoring trait for internal regression metrics
pub trait PerformanceMonitor: Send + Sync {
    /// Record operation timing
    fn record_operation_time(&self, operation: &str, duration_ms: u64);

    /// Record memory usage
    fn record_memory_usage(&self, operation: &str, bytes: usize);

    /// Get performance report
    fn get_performance_report(&self) -> serde_json::Value;

    /// Check if performance targets are met
    fn meets_performance_targets(&self) -> bool;
}
