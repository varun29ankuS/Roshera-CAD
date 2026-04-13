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

// AI-accessible primitives
pub mod box_primitive;
pub mod cone_primitive;
pub mod cylinder_primitive;
pub mod sphere_primitive;
pub mod torus_primitive;
// TODO: Implement remaining primitives
// pub mod pyramid_primitive;
// pub mod wedge_primitive;
// pub mod prism_primitive;

// DEPRECATED: This module alias is kept for backward compatibility
// All new code should use `topology_builder` directly
// TODO: Phase out this alias by updating all references throughout the codebase
pub mod builder {
    pub use super::topology_builder::*;
}

#[cfg(test)]
pub mod primitive_tests;
//
// // AI-Accessible Public Interface
// pub use ai_primitive_registry::{PrimitiveRegistry, AICommand, AIResponse};
// pub use natural_language_schemas::{PrimitiveSchema, ParameterInfo, CommandExample};
// pub use primitive_traits::{Primitive, PrimitiveError, ValidationReport};
//
// /// The main entry point for AI agents to discover and use CAD primitives
// ///
// /// This provides a self-documenting, schema-driven interface that AI systems
// /// can introspect and use without human-written documentation.
// ///
// /// # AI Usage Examples
// /// ```rust
// /// // AI can discover all available primitives
// /// let available = PrimitiveRegistry::list_all_primitives();
// ///
// /// // Get natural language descriptions
// /// let box_info = PrimitiveRegistry::get_primitive_info("box").unwrap();
// /// println!("{}", box_info.natural_description); // "Creates a rectangular box with width, height, and depth"
// ///
// /// // Execute commands with natural language
// /// let command = AICommand::from_text("create a box with width 10, height 5, depth 3");
// /// let result = PrimitiveRegistry::execute_command(command, &mut model)?;
// /// ```
// pub struct PrimitiveSystem;
//
// impl PrimitiveSystem {
//     /// Get all available primitive types with AI-friendly metadata
//     ///
//     /// Returns comprehensive information that AI systems can use to:
//     /// - Understand what primitives are available
//     /// - Learn the parameters for each primitive
//     /// - See examples of how to use them
//     /// - Get natural language descriptions
//     pub fn get_ai_catalog() -> serde_json::Value {
//         PrimitiveRegistry::get_full_catalog()
//     }
//
//     /// Execute a command described in natural language
//     ///
//     /// # Arguments
//     /// * `description` - Natural language description like "create a sphere with radius 5"
//     /// * `model` - The B-Rep model to modify
//     ///
//     /// # Returns
//     /// * `Ok(AIResponse)` - Success with created geometry info
//     /// * `Err(PrimitiveError)` - Detailed error with suggestions
//     ///
//     /// # AI-Friendly Features
//     /// - Fuzzy parameter matching (e.g., "size" → "radius")
//     /// - Unit conversion (e.g., "5 inches" → 127.0)
//     /// - Error messages with correction suggestions
//     /// - Context-aware parameter inference
//     pub fn execute_natural_language(
//         description: &str,
//         model: &mut crate::primitives::topology_builder::BRepModel
//     ) -> Result<AIResponse, PrimitiveError> {
//         PrimitiveRegistry::execute_natural_language(description, model)
//     }
//
//     /// Get examples for a specific primitive type
//     ///
//     /// Returns AI-training-ready examples with:
//     /// - Input variations (different ways to express the same thing)
//     /// - Expected outputs
//     /// - Common error cases
//     /// - Parameter validation rules
//     pub fn get_examples_for_primitive(primitive_type: &str) -> Vec<serde_json::Value> {
//         PrimitiveRegistry::get_examples(primitive_type)
//     }
// }
