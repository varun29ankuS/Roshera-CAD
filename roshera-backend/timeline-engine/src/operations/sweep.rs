//! Sweep operation implementation
//!
//! Creates a solid by sweeping a profile along a path

use super::common::{brep_to_entity_state, entity_state_to_brep};
use crate::{
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationOutputs,
    TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::Tolerance,
    primitives::topology_builder::{BRepModel, GeometryId as GeometryEngineId},
};

/// Implementation of sweep operation
pub struct SweepOp;

#[async_trait]
impl OperationImpl for SweepOp {
    fn operation_type(&self) -> &'static str {
        "sweep"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Sweep { profile, path } = operation {
            // Validate profile exists
            if !context.entity_exists(*profile) {
                return Err(TimelineError::ValidationError(format!(
                    "Profile {:?} not found",
                    profile
                )));
            }

            // Validate path exists
            if !context.entity_exists(*path) {
                return Err(TimelineError::ValidationError(format!(
                    "Path {:?} not found",
                    path
                )));
            }

            // TODO: Add options validation when Operation::Sweep includes options field

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Sweep operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Sweep { profile, path } = operation {
            // Create a new BRep model for the result
            let mut result_brep = BRepModel::new();

            // Get the profile and path geometries
            let profile_entity = context.get_entity(*profile)?;
            let _profile_brep = entity_state_to_brep(&profile_entity)?;

            let path_entity = context.get_entity(*path)?;
            let _path_brep = entity_state_to_brep(&path_entity)?;

            // Use default values since Operation::Sweep doesn't include options
            let twist_angle: f64 = 0.0;
            let scale_factor: f64 = 1.0;

            // Create sweep operation using geometry-engine
            use geometry_engine::operations::sweep::{
                sweep_profile, SweepOptions as GeomSweepOptions,
            };

            // Get the profile and path faces/curves from the BRep
            let profile_face_id = {
                // Assuming profile is a face entity
                let _profile_entity = context.get_entity(*profile)?;
                let mapping = get_entity_mapping();
                mapping
                    .get_geometry_id(*profile)
                    .and_then(|id| match id {
                        GeometryEngineId::Face(face_id) => Some(face_id),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        TimelineError::ValidationError("Profile must be a face".to_string())
                    })?
            };

            let path_edge_id = {
                // Assuming path is an edge entity
                let _path_entity = context.get_entity(*path)?;
                let mapping = get_entity_mapping();
                mapping
                    .get_geometry_id(*path)
                    .and_then(|id| match id {
                        GeometryEngineId::Edge(edge_id) => Some(edge_id),
                        _ => None,
                    })
                    .ok_or_else(|| {
                        TimelineError::ValidationError("Path must be an edge".to_string())
                    })?
            };

            // Import sweep types
            use geometry_engine::operations::sweep::{
                OrientationControl, ScaleControl, SweepType, TwistControl,
            };

            // Create sweep options
            let sweep_options = GeomSweepOptions {
                common: geometry_engine::operations::CommonOptions {
                    tolerance: Tolerance::default(),
                    validate_result: true,
                    merge_entities: true,
                    track_history: false,
                },
                sweep_type: SweepType::Path,
                orientation: OrientationControl::Frenet,
                scale: if (scale_factor - 1.0).abs() > 0.001 {
                    ScaleControl::Linear(1.0, scale_factor)
                } else {
                    ScaleControl::Constant
                },
                twist: if twist_angle.abs() > 0.001 {
                    TwistControl::Linear(twist_angle)
                } else {
                    TwistControl::None
                },
                create_solid: true,
                quality: geometry_engine::operations::sweep::SweepQuality::Standard,
            };

            // Get the edges from the profile face
            let profile_edges = if let Some(face) = result_brep.faces.get(profile_face_id) {
                // Get the outer loop edges
                if let Some(loop_) = result_brep.loops.get(face.outer_loop) {
                    loop_.edges.clone()
                } else {
                    vec![]
                }
            } else {
                return Err(TimelineError::ExecutionError(
                    "Profile face not found".to_string(),
                ));
            };

            // Perform the sweep operation
            let solid_geom_id =
                sweep_profile(&mut result_brep, profile_edges, path_edge_id, sweep_options)
                    .map_err(|e| TimelineError::ExecutionError(format!("Sweep failed: {:?}", e)))?;

            // The solid ID is returned directly
            let solid_id = solid_geom_id;

            // Create entity for the result
            let entity_id = EntityId::new();
            let entity_state = brep_to_entity_state(
                &result_brep,
                entity_id,
                EntityType::Solid,
                Some(format!("Sweep_{}", entity_id.0)),
            )?;

            // Register in entity mapping
            let mapping = get_entity_mapping();
            mapping.register_solid(entity_id, solid_id);

            // Add properties
            let mut final_entity = entity_state;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                obj.insert("operation".to_string(), serde_json::json!("sweep"));
                obj.insert("profile_id".to_string(), serde_json::json!(profile));
                obj.insert("path_id".to_string(), serde_json::json!(path));
                obj.insert("twist_angle".to_string(), serde_json::json!(twist_angle));
                obj.insert("scale_factor".to_string(), serde_json::json!(scale_factor));
            }

            // Add to context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![CreatedEntity {
                    id: entity_id,
                    entity_type: EntityType::Solid,
                    name: Some(format!("Sweep_{}", entity_id.0)),
                }],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Sweep operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, _operation: &Operation) -> ResourceEstimate {
        ResourceEstimate {
            entities_created: 1, // Sweep creates one solid
            entities_modified: 0,
            memory_bytes: 200000,
            time_ms: 200,
        }
    }
}
