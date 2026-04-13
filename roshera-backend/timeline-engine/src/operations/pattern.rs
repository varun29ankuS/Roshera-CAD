//! Pattern operation implementation
//!
//! Creates arrays of geometry (linear, circular, or custom patterns)

use super::common::{brep_to_entity_state, entity_state_to_brep};
use crate::{
    brep_serialization::deserialize_brep,
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationInputs, OperationOutputs, PatternType,
    TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::{Matrix4, Point3, Vector3},
    primitives::{
        solid::SolidId,
        topology_builder::{BRepModel, GeometryId as GeometryEngineId, TopologyBuilder},
    },
};

/// Implementation of pattern operation
pub struct PatternOp;

#[async_trait]
impl OperationImpl for PatternOp {
    fn operation_type(&self) -> &'static str {
        "pattern"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Pattern {
            features,
            pattern_type,
        } = operation
        {
            // Validate features exist
            for feature_id in features {
                if !context.entity_exists(*feature_id) {
                    return Err(TimelineError::ValidationError(format!(
                        "Feature entity {:?} not found",
                        feature_id
                    )));
                }
            }

            // Validate features not empty
            if features.is_empty() {
                return Err(TimelineError::ValidationError(
                    "Pattern requires at least one feature".to_string(),
                ));
            }

            // Validate pattern type specific parameters
            match pattern_type {
                PatternType::Linear {
                    direction,
                    spacing,
                    count,
                } => {
                    // Validate direction vector
                    let dir_magnitude = (direction[0] * direction[0]
                        + direction[1] * direction[1]
                        + direction[2] * direction[2])
                        .sqrt();
                    if dir_magnitude < 0.001 {
                        return Err(TimelineError::ValidationError(
                            "Linear pattern direction vector is too small".to_string(),
                        ));
                    }

                    // Validate spacing
                    if *spacing <= 0.0 {
                        return Err(TimelineError::ValidationError(format!(
                            "Linear pattern spacing must be positive, got {}",
                            spacing
                        )));
                    }

                    // Validate count
                    if *count < 2 {
                        return Err(TimelineError::ValidationError(format!(
                            "Pattern count must be at least 2, got {}",
                            count
                        )));
                    }
                }
                PatternType::Circular { axis, count, angle } => {
                    // Validate axis direction vector
                    let axis_magnitude = (axis.direction[0] * axis.direction[0]
                        + axis.direction[1] * axis.direction[1]
                        + axis.direction[2] * axis.direction[2])
                        .sqrt();
                    if axis_magnitude < 0.001 {
                        return Err(TimelineError::ValidationError(
                            "Circular pattern axis vector is too small".to_string(),
                        ));
                    }

                    // Validate angle
                    if angle.abs() < 0.001 {
                        return Err(TimelineError::ValidationError(
                            "Circular pattern angle is too small".to_string(),
                        ));
                    }

                    // Validate count
                    if *count < 2 {
                        return Err(TimelineError::ValidationError(format!(
                            "Pattern count must be at least 2, got {}",
                            count
                        )));
                    }
                }
                PatternType::Rectangular {
                    x_direction,
                    y_direction,
                    x_spacing,
                    y_spacing,
                    x_count,
                    y_count,
                } => {
                    // Validate X direction vector
                    let x_magnitude = (x_direction[0] * x_direction[0]
                        + x_direction[1] * x_direction[1]
                        + x_direction[2] * x_direction[2])
                        .sqrt();
                    if x_magnitude < 0.001 {
                        return Err(TimelineError::ValidationError(
                            "Rectangular pattern X direction vector is too small".to_string(),
                        ));
                    }

                    // Validate Y direction vector
                    let y_magnitude = (y_direction[0] * y_direction[0]
                        + y_direction[1] * y_direction[1]
                        + y_direction[2] * y_direction[2])
                        .sqrt();
                    if y_magnitude < 0.001 {
                        return Err(TimelineError::ValidationError(
                            "Rectangular pattern Y direction vector is too small".to_string(),
                        ));
                    }

                    // Validate spacings
                    if *x_spacing <= 0.0 {
                        return Err(TimelineError::ValidationError(format!(
                            "X spacing must be positive, got {}",
                            x_spacing
                        )));
                    }
                    if *y_spacing <= 0.0 {
                        return Err(TimelineError::ValidationError(format!(
                            "Y spacing must be positive, got {}",
                            y_spacing
                        )));
                    }

                    // Validate counts
                    if *x_count < 1 {
                        return Err(TimelineError::ValidationError(format!(
                            "X count must be at least 1, got {}",
                            x_count
                        )));
                    }
                    if *y_count < 1 {
                        return Err(TimelineError::ValidationError(format!(
                            "Y count must be at least 1, got {}",
                            y_count
                        )));
                    }
                }
            }

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Pattern operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Pattern {
            features,
            pattern_type,
        } = operation
        {
            // Process each feature to pattern
            let mut all_created_entities = Vec::new();
            let mapping = get_entity_mapping();

            for feature_id in features {
                // Get the feature entity
                let source_entity = context.get_entity(*feature_id)?;
                let mut created_entities = Vec::new();

                match pattern_type {
                    PatternType::Linear {
                        direction,
                        spacing,
                        count,
                    } => {
                        let dir = Vector3::new(direction[0], direction[1], direction[2])
                            .normalize()
                            .unwrap_or(Vector3::X);

                        for i in 1..(*count as usize) {
                            // Calculate translation for this instance
                            let offset = dir * (*spacing * i as f64);
                            let transform = Matrix4::from_translation(&offset);

                            // Clone the source BRep
                            let mut instance_brep = deserialize_brep(
                                &String::from_utf8(source_entity.geometry_data.clone()).map_err(
                                    |e| {
                                        TimelineError::DeserializationError(format!(
                                            "Invalid UTF-8: {}",
                                            e
                                        ))
                                    },
                                )?,
                            )?;

                            // Apply transformation to the geometry
                            use geometry_engine::math::Tolerance;
                            use geometry_engine::operations::transform::{
                                transform_solid, TransformOptions,
                            };
                            use geometry_engine::operations::CommonOptions;

                            // Find the solid in the BRep and transform it
                            let solid_id = instance_brep.solids.iter().next().map(|(id, _)| id);
                            if let Some(solid_id) = solid_id {
                                let transform_options = TransformOptions {
                                    common: CommonOptions {
                                        tolerance: Tolerance::default(),
                                        validate_result: false,
                                        merge_entities: false,
                                        track_history: false,
                                    },
                                    copy: false,
                                    update_parameterization: false,
                                };
                                let _ = transform_solid(
                                    &mut instance_brep,
                                    solid_id,
                                    transform,
                                    transform_options,
                                );
                            }

                            // Create entity for this instance
                            let entity_id = EntityId::new();
                            let entity_state = brep_to_entity_state(
                                &instance_brep,
                                entity_id,
                                source_entity.entity_type,
                                Some(format!("Pattern_Instance_{}_{}", feature_id.0, i)),
                            )?;

                            // Add to context
                            context.add_temp_entity(entity_state)?;

                            created_entities.push(CreatedEntity {
                                id: entity_id,
                                entity_type: source_entity.entity_type,
                                name: Some(format!("Pattern_Instance_{}_{}", feature_id.0, i)),
                            });
                        }
                    }
                    PatternType::Circular { axis, count, angle } => {
                        let axis_vec =
                            Vector3::new(axis.direction[0], axis.direction[1], axis.direction[2])
                                .normalize()
                                .unwrap_or(Vector3::Z);
                        let center_pt = Point3::new(axis.origin[0], axis.origin[1], axis.origin[2]);

                        let angle_per_instance = angle / (*count - 1) as f64;

                        for i in 1..(*count as usize) {
                            // Calculate rotation for this instance
                            let rotation_angle = angle_per_instance * i as f64;
                            let transform =
                                Matrix4::from_axis_angle(&axis_vec, rotation_angle.to_radians())
                                    .unwrap_or(Matrix4::IDENTITY);

                            // Clone the source BRep
                            let mut instance_brep = deserialize_brep(
                                &String::from_utf8(source_entity.geometry_data.clone()).map_err(
                                    |e| {
                                        TimelineError::DeserializationError(format!(
                                            "Invalid UTF-8: {}",
                                            e
                                        ))
                                    },
                                )?,
                            )?;

                            // Apply transformation to the geometry
                            use geometry_engine::math::Tolerance;
                            use geometry_engine::operations::transform::{
                                transform_solid, TransformOptions,
                            };
                            use geometry_engine::operations::CommonOptions;

                            // Find the solid in the BRep and transform it
                            let solid_id = instance_brep.solids.iter().next().map(|(id, _)| id);
                            if let Some(solid_id) = solid_id {
                                let transform_options = TransformOptions {
                                    common: CommonOptions {
                                        tolerance: Tolerance::default(),
                                        validate_result: false,
                                        merge_entities: false,
                                        track_history: false,
                                    },
                                    copy: false,
                                    update_parameterization: false,
                                };
                                let _ = transform_solid(
                                    &mut instance_brep,
                                    solid_id,
                                    transform,
                                    transform_options,
                                );
                            }

                            // Create entity for this instance
                            let entity_id = EntityId::new();
                            let entity_state = brep_to_entity_state(
                                &instance_brep,
                                entity_id,
                                source_entity.entity_type,
                                Some(format!("Pattern_Circular_{}_{}", feature_id.0, i)),
                            )?;

                            // Add to context
                            context.add_temp_entity(entity_state)?;

                            created_entities.push(CreatedEntity {
                                id: entity_id,
                                entity_type: source_entity.entity_type,
                                name: Some(format!("Pattern_Circular_{}_{}", feature_id.0, i)),
                            });
                        }
                    }
                    PatternType::Rectangular {
                        x_direction,
                        y_direction,
                        x_spacing,
                        y_spacing,
                        x_count,
                        y_count,
                    } => {
                        // Generate rectangular pattern positions
                        for x_idx in 0..(*x_count as usize) {
                            for y_idx in 0..(*y_count as usize) {
                                // Skip the original position (0,0)
                                if x_idx == 0 && y_idx == 0 {
                                    continue;
                                }

                                // Calculate offset
                                let x_dir =
                                    Vector3::new(x_direction[0], x_direction[1], x_direction[2])
                                        .normalize()
                                        .unwrap_or(Vector3::X);
                                let y_dir =
                                    Vector3::new(y_direction[0], y_direction[1], y_direction[2])
                                        .normalize()
                                        .unwrap_or(Vector3::Y);

                                let offset = x_dir * (*x_spacing * x_idx as f64)
                                    + y_dir * (*y_spacing * y_idx as f64);

                                let transform = Matrix4::from_translation(&offset);

                                // Clone the source BRep
                                let mut instance_brep = deserialize_brep(
                                    &String::from_utf8(source_entity.geometry_data.clone())
                                        .map_err(|e| {
                                            TimelineError::DeserializationError(format!(
                                                "Invalid UTF-8: {}",
                                                e
                                            ))
                                        })?,
                                )?;

                                // Apply transformation to the geometry
                                use geometry_engine::math::Tolerance;
                                use geometry_engine::operations::transform::{
                                    transform_solid, TransformOptions,
                                };
                                use geometry_engine::operations::CommonOptions;

                                // Find the solid in the BRep and transform it
                                let solid_id = instance_brep.solids.iter().next().map(|(id, _)| id);
                                if let Some(solid_id) = solid_id {
                                    let transform_options = TransformOptions {
                                        common: CommonOptions {
                                            tolerance: Tolerance::default(),
                                            validate_result: false,
                                            merge_entities: false,
                                            track_history: false,
                                        },
                                        copy: false,
                                        update_parameterization: false,
                                    };
                                    let _ = transform_solid(
                                        &mut instance_brep,
                                        solid_id,
                                        transform,
                                        transform_options,
                                    );
                                }

                                // Create entity for this instance
                                let entity_id = EntityId::new();
                                let entity_state = brep_to_entity_state(
                                    &instance_brep,
                                    entity_id,
                                    source_entity.entity_type,
                                    Some(format!(
                                        "Pattern_Rectangular_{}_{}_{}",
                                        feature_id.0, x_idx, y_idx
                                    )),
                                )?;

                                // Add to context
                                context.add_temp_entity(entity_state)?;

                                created_entities.push(CreatedEntity {
                                    id: entity_id,
                                    entity_type: source_entity.entity_type,
                                    name: Some(format!(
                                        "Pattern_Rectangular_{}_{}_{}",
                                        feature_id.0, x_idx, y_idx
                                    )),
                                });
                            }
                        }
                    }
                }

                all_created_entities.extend(created_entities);
            }

            // Update context
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: all_created_entities,
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Pattern operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Pattern {
            features,
            pattern_type,
        } = operation
        {
            // Estimate based on number of features and pattern complexity
            let feature_count = features.len();
            let pattern_count = match pattern_type {
                PatternType::Linear { count, .. } => *count as usize,
                PatternType::Circular { count, .. } => *count as usize,
                PatternType::Rectangular {
                    x_count, y_count, ..
                } => (*x_count * *y_count) as usize,
            };

            ResourceEstimate {
                entities_created: feature_count * pattern_count,
                entities_modified: 0,
                memory_bytes: (feature_count * pattern_count * 50000) as u64,
                time_ms: (feature_count * pattern_count * 10) as u64,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}
