//! Core data structures for Roshera CAD
//!
//! This crate provides the fundamental types used throughout the Roshera CAD system,
//! including geometry representations, commands, materials, and error types.

#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]
// Allow pedantic warnings during development - they're suggestions not errors
#![allow(clippy::pedantic)]
#![allow(clippy::module_name_repetitions)]
#![allow(clippy::must_use_candidate)]
#![allow(clippy::return_self_not_must_use)]
#![allow(clippy::missing_errors_doc)]
#![allow(clippy::missing_panics_doc)]

pub mod api;
pub mod commands;
pub mod error;
pub mod geometry;
pub mod geometry_commands;
pub mod hierarchy;
pub mod materials;
pub mod scene_state;
pub mod session;
pub mod system_context;
pub mod timeline_types;
pub mod traits;
pub mod vision;

pub use api::*;
// Re-export commands types, excluding CommandContext (conflicts with system_context::CommandContext)
// and ExportFormat/ExportOptions (conflicts with geometry_commands re-exports above).
pub use commands::{
    AICommand, AnalysisType, CommandResult, SessionAction, TransformType, ViewState, ViewType,
};
pub use error::*;
pub use geometry::*;
pub use geometry_commands::{
    AnalysisResult, AngleUnit, AppearanceSettings, AxisType, Command, EdgeSelection, EdgeType,
    EntityReference, ExportFormat, ExportOptions, ExportUnits, ExtrusionParams, FilletParams,
    HolePosition, InterferencePair, LoftOptions, MateType, MaterialProperties, OffsetType,
    PatternType, PlaneType, PrimitiveParams, QueryResult, QueryType, RibDirection, SectionPlane,
    SketchConstraintType, SketchPlane, SweepOptions, ThreadSpecification, ThreadStandard,
    Transform as GeometryTransform, TransformOp,
};
pub use materials::*;
// Re-export session types, excluding UserInfo which conflicts with system_context::UserInfo
pub use session::{
    CollaborationEvent, CubeFace, GridSettings, HistoryEntry, OrientationCubeState,
    SessionSettings, SessionState, SketchPlaneInfo, SketchState, Units, UserRole,
};
// Re-export scene_state types with specific names to avoid conflicts
pub use scene_state::{
    BoundingBox as SceneBoundingBox, CameraState, GridSettings as SceneGridSettings,
    MassProperties, MaterialRef, ObjectChanges, ObjectProperties, ObjectType, ProjectionType,
    Quaternion, RelationshipType, SceneFilters, SceneMetadata, SceneObject, SceneQuery,
    SceneQueryType, SceneState, SceneStatistics, SceneUpdate, SelectionMode, SelectionState,
    SpatialRelationship, Transform3D as SceneTransform3D, UnitSystem, Viewport,
};

// Re-export system context types
pub use system_context::*;

// Re-export traits for cross-module interfaces
pub use traits::{
    GeometricEntity, IdConvertible, MaterialConvertible, MeshConvertible, PerformanceMonitor,
};

// Re-export async traits only for non-WASM targets
#[cfg(not(target_arch = "wasm32"))]
pub use traits::{
    AICommandProcessor, ExportOperations, GeometryOperations, SessionOperations, TimelineExecutable,
};

// Re-export timeline types
pub use timeline_types::{
    BranchId, EntityReference as TimelineEntityReference, EntityType, EventId, Operation,
    OperationOutputs, TimelineError, TimelineEvent,
};

/// Unique identifier type for objects
pub type ObjectId = uuid::Uuid;

/// Timestamp in milliseconds since Unix epoch
pub type Timestamp = u64;

/// Returns the current wall-clock time as milliseconds since the Unix epoch.
///
/// Falls back to `0` if the system clock is set before `UNIX_EPOCH`, which
/// would otherwise cause `SystemTime::duration_since` to return an error.
/// Timestamps in this crate are audit/metadata fields where producing a
/// monotonically-increasing value is preferable to panicking.
pub fn unix_millis_now() -> Timestamp {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 3D position vector [x, y, z]
pub type Position3D = [f32; 3];

/// 3D vector for directions and offsets
pub type Vector3D = [f32; 3];

/// Color as RGBA values [r, g, b, a] in range 0.0-1.0
pub type Color = [f32; 4];

/// Engineering tolerance for comparisons
pub const TOLERANCE: f32 = 1e-6;

/// Maximum allowed vertices in a mesh
pub const MAX_VERTICES: usize = 1_000_000;

/// Maximum allowed triangles in a mesh
pub const MAX_TRIANGLES: usize = 1_000_000;

/// Utility function to check if two floats are approximately equal
#[inline]
pub fn approx_eq(a: f32, b: f32) -> bool {
    (a - b).abs() < TOLERANCE
}

/// Utility function to check if a vector is approximately zero
#[inline]
pub fn approx_zero(v: &Vector3D) -> bool {
    v.iter().all(|&x| x.abs() < TOLERANCE)
}

/// Result type for Roshera operations
pub type RosheraResult<T> = Result<T, RosheraError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_approx_eq() {
        assert!(approx_eq(1.0, 1.0 + TOLERANCE / 2.0));
        assert!(!approx_eq(1.0, 1.0 + TOLERANCE * 2.0));
    }

    #[test]
    fn test_approx_zero() {
        assert!(approx_zero(&[0.0, 0.0, 0.0]));
        assert!(approx_zero(&[TOLERANCE / 2.0, 0.0, 0.0]));
        assert!(!approx_zero(&[1.0, 0.0, 0.0]));
    }
}
