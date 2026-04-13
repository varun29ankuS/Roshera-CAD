//! RosheraCAD B-Rep Geometry Engine
//!
//! A high-performance boundary representation (B-Rep) engine for aerospace CAD applications.

pub mod assembly;
pub mod math;
pub mod operations;
pub mod performance;
pub mod primitives;
pub mod sketch2d;
pub mod tessellation;

#[cfg(feature = "export")]
pub mod export;

// Re-export commonly used types
pub use math::{Matrix4, Point3, Tolerance, Vector3};
pub use primitives::topology_builder::BRepModel;

/// Prelude module for convenient imports
pub mod prelude {
    pub use crate::assembly::{
        Assembly, AssemblyError, Component, ComponentId, ComponentProperties, ExplodedViewConfig,
        ExplosionStep, MateConstraint, MateId, MateReference, MateType, MotionLimits,
    };
    pub use crate::math::{Matrix4, Point3, Tolerance, Vector3, NORMAL_TOLERANCE};
    pub use crate::operations::{
        boolean_operation, chamfer_edges, extrude_face, fillet_edges, mirror, revolve_face, rotate,
        scale, transform_solid, translate, BooleanOp, BooleanOptions, ChamferOptions,
        ExtrudeOptions, FilletOptions, RevolveOptions, TransformOptions,
    };
    pub use crate::primitives::{
        edge::EdgeId, face::FaceId, shell::ShellId, solid::SolidId, topology_builder::BRepModel,
        vertex::VertexId,
    };
    pub use crate::tessellation::{
        tessellate_solid, TessellationParams, ThreeJsMesh, TriangleMesh,
    };
}
