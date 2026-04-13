//! Chamfer operation implementation
//!
//! Creates beveled edges on a solid

use super::common::{brep_to_entity_state, entity_state_to_brep, validate_edges_same_solid};
use crate::{
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    EntityId, EntityType, ModifiedEntity, Operation, OperationInputs, OperationOutputs,
    TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::Tolerance,
    operations::chamfer::ChamferOptions,
    primitives::{
        edge::EdgeId,
        solid::SolidId,
        topology_builder::{BRepModel, GeometryId as GeometryEngineId},
    },
};

/// Implementation of chamfer operation
pub struct ChamferOp;

#[async_trait]
impl OperationImpl for ChamferOp {
    fn operation_type(&self) -> &'static str {
        "chamfer"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Chamfer {
            edges,
            distance,
            angle,
        } = operation
        {
            // Check we have edges
            if edges.is_empty() {
                return Err(TimelineError::ValidationError(
                    "Chamfer requires at least one edge".to_string(),
                ));
            }

            // Validate distance
            if *distance <= 0.0 {
                return Err(TimelineError::ValidationError(format!(
                    "Chamfer distance must be positive, got {}",
                    distance
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

            // Chamfer validation complete - Operation::Chamfer has edges, distance and angle fields

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Chamfer operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Chamfer {
            edges,
            distance,
            angle,
        } = operation
        {
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

            // Use chamfer parameters from Operation::Chamfer
            let chamfer_angle = angle.unwrap_or(45.0);
            let two_distances = (*distance, *distance); // Use same distance for both sides

            // Apply chamfer operation using geometry-engine
            use geometry_engine::operations::chamfer::{
                chamfer_edges, ChamferOptions as GeomChamferOptions, ChamferType,
            };

            // Import PropagationMode
            use geometry_engine::operations::chamfer::PropagationMode;

            // Create chamfer options
            let chamfer_options = GeomChamferOptions {
                common: geometry_engine::operations::CommonOptions {
                    tolerance: Tolerance::default(),
                    validate_result: true,
                    merge_entities: true,
                    track_history: false,
                },
                chamfer_type: if let Some(angle_val) = angle {
                    // If angle is specified, use angle-based chamfer
                    ChamferType::Angle(*angle_val)
                } else {
                    // Otherwise use equal distance chamfer
                    ChamferType::EqualDistance(*distance)
                },
                distance1: *distance,
                distance2: *distance, // Symmetric chamfer
                symmetric: true,
                propagation: PropagationMode::Tangent,
                preserve_edges: true,
            };

            // Get the solid ID from the BRep (there should be one solid)
            let solid_id = if let Some((solid_id, _)) = brep.solids.iter().next() {
                solid_id
            } else {
                return Err(TimelineError::ExecutionError(
                    "No solid found in BRep".to_string(),
                ));
            };

            // Apply the chamfer operation
            let result = chamfer_edges(&mut brep, solid_id, edge_ids, chamfer_options);

            // Check if operation succeeded
            if let Err(e) = result {
                return Err(TimelineError::ExecutionFailed(format!(
                    "Chamfer operation failed: {:?}",
                    e
                )));
            }

            // Update the entity state
            let updated_entity = brep_to_entity_state(
                &brep,
                solid_entity_id,
                EntityType::Solid,
                Some(format!("Chamfered_Solid_{}", solid_entity_id.0)),
            )?;

            // Add operation metadata
            let mut final_entity = updated_entity;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                // Get existing chamfer count or 0
                let chamfer_count = obj
                    .get("chamfer_count")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0)
                    + 1;

                obj.insert("last_operation".to_string(), serde_json::json!("chamfer"));
                obj.insert(
                    "chamfer_count".to_string(),
                    serde_json::json!(chamfer_count),
                );
                obj.insert(
                    "last_chamfer_distance".to_string(),
                    serde_json::json!(distance),
                );
                obj.insert("last_chamfer_angle".to_string(), serde_json::json!(angle));
                obj.insert(
                    "last_chamfer_edges".to_string(),
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
                "Expected Chamfer operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Chamfer { edges, .. } = operation {
            ResourceEstimate {
                entities_created: 0, // Chamfer modifies existing entity
                entities_modified: 1,
                memory_bytes: edges.len() as u64 * 8000,
                time_ms: edges.len() as u64 * 40,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}
