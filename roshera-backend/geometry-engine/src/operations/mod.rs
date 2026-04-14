//! B-Rep Operations Module
//!
//! World-class implementations of all standard CAD operations on B-Rep models.
//! All operations maintain exact analytical geometry (no tessellation).
//!
//! # Design Principles
//!
//! 1. **Analytical Precision**: All operations work on exact NURBS/analytical surfaces
//! 2. **Topological Integrity**: Maintain watertight B-Rep at all times
//! 3. **Performance**: Match or exceed Parasolid/ACIS performance targets
//! 4. **Thread Safety**: All operations are thread-safe and parallelizable
//! 5. **History Tracking**: Full parametric history for all operations
//!
//! # Performance Targets
//!
//! - Boolean operations: < 100ms for 10k faces
//! - Fillet/chamfer: < 50ms per edge
//! - Extrude/revolve: < 20ms
//! - Loft: < 200ms for 10 profiles
//! - Pattern: < 10ms per instance

// Core operation modules
pub mod blend;
pub mod boolean;
pub mod chamfer;
pub mod deep_clone;
pub mod delete;
pub mod draft;
pub mod extrude;
pub mod fillet;
pub mod fillet_robust;
pub mod g2_blending;
pub mod loft;
pub mod modify;
pub mod offset;
pub mod pattern;
pub mod revolve;
pub mod sweep;
pub mod transform;

// Utility modules
pub mod imprint;
pub mod intersect;
pub mod project;
pub mod sew;
pub mod split;
pub mod surface_intersection;

// AI integration
pub mod ai_operations_registry;

// Test modules
//#[cfg(test)]
//mod tests;

// Re-export commonly used types
pub use blend::{blend_faces, BlendOptions};
pub use boolean::{boolean_operation, BooleanOp, BooleanOptions};
pub use chamfer::{chamfer_edges, ChamferOptions};
pub use draft::{apply_draft, DraftOptions};
pub use extrude::{extrude_face, extrude_profile, ExtrudeOptions};
pub use fillet::{fillet_edges, fillet_vertices, FilletOptions};
pub use g2_blending::{BlendingComplexity, G2BlendingOperations, G2QualityReport};
pub use loft::{loft_profiles, LoftOptions};
pub use offset::{offset_face, offset_solid, OffsetOptions};
pub use pattern::{create_pattern, PatternOptions, PatternType};
pub use revolve::{revolve_face, revolve_profile, RevolveOptions};
pub use sweep::{sweep_profile, SweepOptions};
pub use transform::{
    mirror, rotate, scale, transform_edges, transform_faces, transform_solid, translate,
    TransformOptions, TransformResult,
};

use crate::math::Tolerance;

/// Common result type for all operations
pub type OperationResult<T> = Result<T, OperationError>;

/// Comprehensive error types for B-Rep operations
#[derive(Debug, Clone, PartialEq)]
pub enum OperationError {
    /// Invalid input geometry
    InvalidGeometry(String),

    /// Topology error (non-manifold, etc.)
    TopologyError(String),

    /// Numerical computation error
    NumericalError(String),

    /// Operation would create self-intersection
    SelfIntersection,

    /// Operation would create invalid B-Rep
    InvalidBRep(String),

    /// Feature too small for tolerance
    FeatureTooSmall,

    /// Operation not yet implemented
    NotImplemented(String),

    /// Internal algorithm error
    InternalError(String),

    /// Intersection computation failed
    IntersectionFailed,

    /// Cannot create blend/fillet with given radius
    InvalidRadius(f64),

    /// Profile is not closed for solid operations
    OpenProfile,

    /// Incompatible profiles for lofting
    IncompatibleProfiles,

    /// Invalid pattern parameters
    InvalidPattern(String),

    /// Invalid input provided
    InvalidInput {
        parameter: String,
        expected: String,
        received: String,
    },
}

impl From<crate::math::MathError> for OperationError {
    fn from(err: crate::math::MathError) -> Self {
        OperationError::NumericalError(format!("{:?}", err))
    }
}

impl std::fmt::Display for OperationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OperationError::InvalidGeometry(msg) => write!(f, "Invalid geometry: {}", msg),
            OperationError::TopologyError(msg) => write!(f, "Topology error: {}", msg),
            OperationError::NumericalError(msg) => write!(f, "Numerical error: {}", msg),
            OperationError::SelfIntersection => {
                write!(f, "Operation would create self-intersection")
            }
            OperationError::InvalidBRep(msg) => write!(f, "Invalid B-Rep: {}", msg),
            OperationError::FeatureTooSmall => write!(f, "Feature too small for current tolerance"),
            OperationError::NotImplemented(msg) => write!(f, "Not implemented: {}", msg),
            OperationError::InternalError(msg) => write!(f, "Internal error: {}", msg),
            OperationError::IntersectionFailed => write!(f, "Intersection computation failed"),
            OperationError::InvalidRadius(r) => write!(f, "Invalid radius: {}", r),
            OperationError::OpenProfile => write!(f, "Profile must be closed for solid operations"),
            OperationError::IncompatibleProfiles => {
                write!(f, "Profiles are incompatible for lofting")
            }
            OperationError::InvalidPattern(msg) => write!(f, "Invalid pattern: {}", msg),
            OperationError::InvalidInput {
                parameter,
                expected,
                received,
            } => write!(
                f,
                "Invalid input for '{}': expected {}, received {}",
                parameter, expected, received
            ),
        }
    }
}

impl std::error::Error for OperationError {}

/// Options common to many operations
#[derive(Debug, Clone)]
pub struct CommonOptions {
    /// Tolerance for the operation
    pub tolerance: Tolerance,

    /// Whether to validate result
    pub validate_result: bool,

    /// Whether to merge coincident entities
    pub merge_entities: bool,

    /// Whether to track operation in history
    pub track_history: bool,
}

impl Default for CommonOptions {
    fn default() -> Self {
        use crate::math::tolerance::NORMAL_TOLERANCE;
        Self {
            tolerance: NORMAL_TOLERANCE,
            validate_result: true,
            merge_entities: true,
            track_history: true,
        }
    }
}

// Re-export commonly used types and functions
pub use delete::{
    delete_edge, delete_entities, delete_face, delete_solid, DeleteOptions, DeleteResult,
    DeleteTarget,
};
pub use modify::{apply_modification, ModifyOptions, ModifyResult, ModifyType};
