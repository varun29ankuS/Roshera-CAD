//! RosheraCAD B-Rep Geometry Engine
//!
//! A high-performance boundary representation (B-Rep) engine for aerospace CAD applications.

pub mod assembly;
pub mod drawing;
pub mod gdt;
pub mod harness;
pub mod labels;
pub mod math;
pub mod operations;
pub mod performance;
pub mod primitives;
pub mod queries;
pub mod readable;
pub mod render;
pub mod sketch2d;
pub mod spatial;
pub mod tessellation;
pub mod units;

#[cfg(feature = "export")]
pub mod export;

// Re-export commonly used types
pub use math::{Matrix4, Point3, Tolerance, Vector3};
pub use primitives::topology_builder::BRepModel;
pub use units::LengthUnit;

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
    pub use crate::readable::{
        format_location_oneliner, DistanceReport, PartProximity, PartReport, PartSummary,
    };
    pub use crate::tessellation::{
        tessellate_solid, TessellationParams, ThreeJsMesh, TriangleMesh,
    };
}
