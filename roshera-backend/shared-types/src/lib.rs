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

pub use api::*;
pub use commands::*;
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
pub use session::*;
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

use serde::{Deserialize, Serialize};

/// Unique identifier type for objects
pub type ObjectId = uuid::Uuid;

/// Timestamp in milliseconds since Unix epoch
pub type Timestamp = u64;

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
