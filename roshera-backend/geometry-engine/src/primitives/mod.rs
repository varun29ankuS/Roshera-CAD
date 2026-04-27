// Core topology modules
pub mod blending_surfaces;
pub mod curve;
pub mod edge;
pub mod face;
pub mod fillet_surfaces;
pub mod r#loop;
pub mod shell;
pub mod solid;
pub mod surface;
pub mod topology;
pub mod topology_builder;
pub mod validation;
pub mod vertex;

// AI-First Primitive System
pub mod ai_primitive_registry;
pub mod natural_language_schemas;
pub mod primitive_examples;
pub mod primitive_traits;

// Geometry intelligence
pub mod feature_recognition;
pub mod geometry_summary;
pub mod tool_schema_generator;

// AI-accessible primitives
pub mod box_primitive;
pub mod cone_primitive;
pub mod cylinder_primitive;
pub mod sphere_primitive;
pub mod torus_primitive;

// `builder` is a re-export alias for `topology_builder` — kept because
// tessellation, export, and ai-integration crates import via this path.
pub mod builder {
    pub use super::topology_builder::*;
}

#[cfg(test)]
pub mod primitive_tests;
