//! Loft operation implementation
//!
//! Creates a solid by lofting between multiple profiles (cross-sections)

use super::common::{brep_to_entity_state, entity_state_to_brep};
use crate::{
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationOutputs, TimelineError,
    TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::Tolerance,
    primitives::topology_builder::{BRepModel, GeometryId as GeometryEngineId},
};

/// Implementation of loft operation
pub struct LoftOp;

#[async_trait]
impl OperationImpl for LoftOp {
    fn operation_type(&self) -> &'static str {
        "loft"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Loft {
            profiles,
            guide_curves: _,
        } = operation
        {
            // Check we have at least 2 profiles
            if profiles.len() < 2 {
                return Err(TimelineError::ValidationError(
                    "Loft requires at least 2 profiles".to_string(),
                ));
            }

            // Validate all profiles exist
            for profile_id in profiles {
                if !context.entity_exists(*profile_id) {
                    return Err(TimelineError::ValidationError(format!(
                        "Profile {:?} not found",
                        profile_id
                    )));
                }
            }

            // Operation::Loft validation complete

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Loft operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Loft {
            profiles,
            guide_curves: _,
        } = operation
        {
            // Create a new BRep model for the result
            let mut result_brep = BRepModel::new();

            // Get the profile geometries
            let mut profile_breps = Vec::new();
            for profile_id in profiles {
                let entity = context.get_entity(*profile_id)?;
                let brep = entity_state_to_brep(&entity)?;
                profile_breps.push(brep);
            }

            // Use high-quality cubic interpolation for professional CAD results
            // Linear interpolation would create faceted surfaces between profiles
            let loft_type_choice = geometry_engine::operations::loft::LoftType::Cubic;

            // Create loft operation using geometry-engine
            use geometry_engine::operations::loft::{
                loft_profiles, LoftOptions as GeomLoftOptions,
            };

            // Get the profile edge loops from the BRep
            let mut profile_edge_loops = Vec::new();
            let mapping = get_entity_mapping();

            for profile_id in profiles {
                let _profile_entity = context.get_entity(*profile_id)?;
                let face_id = mapping
                    .get_geometry_id(*profile_id)
                    .and_then(|id| match id {
                        GeometryEngineId::Face(face_id) => Some(face_id),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        TimelineError::ValidationError(format!(
                            "Profile {:?} must be a face",
                            profile_id
                        ))
                    })?;

                // Get the edges from the face's outer loop
                let edges = if let Some(face) = result_brep.faces.get(face_id) {
                    if let Some(loop_) = result_brep.loops.get(face.outer_loop) {
                        loop_.edges.clone()
                    } else {
                        vec![]
                    }
                } else {
                    return Err(TimelineError::ExecutionError(format!(
                        "Profile face {:?} not found",
                        face_id
                    )));
                };

                profile_edge_loops.push(edges);
            }

            // Import loft types

            // Create loft options with production-grade settings
            let loft_options = GeomLoftOptions {
                common: geometry_engine::operations::CommonOptions {
                    tolerance: Tolerance::default(),
                    validate_result: true,
                    merge_entities: true,
                    track_history: false,
                },
                loft_type: loft_type_choice,
                closed: false,       // Open loft - connect first and last profiles only
                create_solid: true,  // Create watertight solid for manufacturability
                start_tangent: None, // Natural tangency at boundaries
                end_tangent: None,
                guide_curves: vec![],        // No guide curves specified
                vertex_correspondence: None, // Automatic correspondence based on proximity
                sections: 20,                // High resolution for smooth surface quality
            };

            // Perform the loft operation
            let solid_geom_id =
                loft_profiles(&mut result_brep, profile_edge_loops, loft_options)
                    .map_err(|e| TimelineError::ExecutionError(format!("Loft failed: {:?}", e)))?;

            // The solid ID is returned directly
            let solid_id = solid_geom_id;

            // Create entity for the result
            let entity_id = EntityId::new();
            let entity_state = brep_to_entity_state(
                &result_brep,
                entity_id,
                EntityType::Solid,
                Some(format!("Loft_{}", entity_id.0)),
            )?;

            // Register in entity mapping
            let mapping = get_entity_mapping();
            mapping.register_solid(entity_id, solid_id);

            // Add properties documenting the loft parameters for traceability
            let mut final_entity = entity_state;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                obj.insert("operation".to_string(), serde_json::json!("loft"));
                obj.insert(
                    "profile_count".to_string(),
                    serde_json::json!(profiles.len()),
                );
                obj.insert("interpolation".to_string(), serde_json::json!("cubic"));
                obj.insert("closed".to_string(), serde_json::json!(false));
                obj.insert("sections".to_string(), serde_json::json!(20));
                obj.insert("quality".to_string(), serde_json::json!("high"));
            }

            // Add to context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![CreatedEntity {
                    id: entity_id,
                    entity_type: EntityType::Solid,
                    name: Some(format!("Loft_{}", entity_id.0)),
                }],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Loft operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Loft { profiles, .. } = operation {
            ResourceEstimate {
                entities_created: 1, // Loft creates one solid
                entities_modified: 0,
                memory_bytes: profiles.len() as u64 * 100000,
                time_ms: profiles.len() as u64 * 100,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}
