//! Fillet operation implementation
//!
//! Creates rounded edges on a solid

use super::common::{brep_to_entity_state, entity_state_to_brep, validate_edges_same_solid};
use crate::{
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    EntityType, Operation, OperationOutputs, TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::Tolerance, primitives::topology_builder::GeometryId as GeometryEngineId,
};

/// Implementation of fillet operation
pub struct FilletOp;

#[async_trait]
impl OperationImpl for FilletOp {
    fn operation_type(&self) -> &'static str {
        "fillet"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Fillet { edges, radius } = operation {
            // Check we have edges
            if edges.is_empty() {
                return Err(TimelineError::ValidationError(
                    "Fillet requires at least one edge".to_string(),
                ));
            }

            // Validate radius
            if *radius <= 0.0 {
                return Err(TimelineError::ValidationError(format!(
                    "Fillet radius must be positive, got {}",
                    radius
                )));
            }

            // Validate all edges exist
            for edge_id in edges {
                if !context.entity_exists(*edge_id) {
                    return Err(TimelineError::ValidationError(format!(
                        "Edge {:?} not found",
                        edge_id
                    )));
                }
            }

            // Validate edges belong to the same solid
            validate_edges_same_solid(edges, context)?;

            // Fillet validation complete - Operation::Fillet has edges and radius fields

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Fillet operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Fillet { edges, radius } = operation {
            // Find the solid that contains these edges
            let solid_entity_id = validate_edges_same_solid(edges, context)?;

            // Get the solid's BRep
            let solid_entity = context.get_entity(solid_entity_id)?;
            let mut brep = entity_state_to_brep(&solid_entity)?;

            // Get edge IDs from entity mapping
            let mapping = get_entity_mapping();
            let mut edge_ids = Vec::new();
            for edge_entity_id in edges {
                if let Some(geom_id) = mapping.get_geometry_id(*edge_entity_id) {
                    if let GeometryEngineId::Edge(edge_id) = geom_id {
                        edge_ids.push(edge_id);
                    }
                }
            }

            // Apply fillet operation using geometry-engine
            use geometry_engine::operations::fillet::{
                fillet_edges, FilletOptions as GeomFilletOptions, FilletType,
            };

            // Create fillet options
            let fillet_options = GeomFilletOptions {
                common: geometry_engine::operations::CommonOptions {
                    tolerance: Tolerance::default(),
                    validate_result: true,
                    merge_entities: true,
                    track_history: false,
                },
                fillet_type: FilletType::Constant(*radius),
                radius: *radius,
                propagation: geometry_engine::operations::fillet::PropagationMode::Tangent,
                preserve_edges: true,
                quality: geometry_engine::operations::fillet::FilletQuality::Standard,
            };

            // Get the solid ID from the BRep (there should be one solid)
            let solid_id = if let Some((solid_id, _)) = brep.solids.iter().next() {
                solid_id
            } else {
                return Err(TimelineError::ExecutionError(
                    "No solid found in BRep".to_string(),
                ));
            };

            // Apply the fillet operation
            let result = fillet_edges(&mut brep, solid_id, edge_ids, fillet_options);

            // Check if operation succeeded
            if let Err(e) = result {
                return Err(TimelineError::ExecutionFailed(format!(
                    "Fillet operation failed: {:?}",
                    e
                )));
            }

            // Update the entity state
            let updated_entity = brep_to_entity_state(
                &brep,
                solid_entity_id,
                EntityType::Solid,
                Some(format!("Filleted_Solid_{}", solid_entity_id.0)),
            )?;

            // Add operation metadata
            let mut final_entity = updated_entity;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                // Get existing fillet count or 0
                let fillet_count = obj
                    .get("fillet_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    + 1;

                obj.insert("last_operation".to_string(), serde_json::json!("fillet"));
                obj.insert("fillet_count".to_string(), serde_json::json!(fillet_count));
                obj.insert("last_fillet_radius".to_string(), serde_json::json!(radius));
                obj.insert(
                    "last_fillet_edges".to_string(),
                    serde_json::json!(edges.len()),
                );
            }

            // Update context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![],
                modified: vec![solid_entity_id],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Fillet operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Fillet { edges, .. } = operation {
            ResourceEstimate {
                entities_created: 0, // Fillet modifies existing entity
                entities_modified: 1,
                memory_bytes: edges.len() as u64 * 10000,
                time_ms: edges.len() as u64 * 50,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}
