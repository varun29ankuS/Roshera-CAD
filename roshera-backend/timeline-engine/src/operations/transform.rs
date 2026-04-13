//! Transform operation implementation

use super::common::{
    array_to_matrix4, brep_to_entity_state, entity_state_to_brep, validate_transform_matrix,
};
use crate::{
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    EntityId, EntityType, Modification, ModifiedEntity, Operation, OperationInputs,
    OperationOutputs, TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::{Matrix4, Point3, Quaternion, Vector3},
    primitives::{topology_builder::BRepModel, vertex::Vertex},
};

/// Implementation of transform operation
pub struct TransformOp;

#[async_trait]
impl OperationImpl for TransformOp {
    fn operation_type(&self) -> &'static str {
        "transform"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Transform {
            entities,
            transformation,
        } = operation
        {
            // Validate all entities exist
            for entity_id in entities {
                let entity = context.get_entity(*entity_id)?;

                // Validate it's a transformable entity type
                match entity.entity_type {
                    EntityType::Solid
                    | EntityType::Surface
                    | EntityType::Curve
                    | EntityType::Sketch => {}
                    _ => {
                        return Err(TimelineError::ValidationError(format!(
                            "Entity type {:?} cannot be transformed",
                            entity.entity_type
                        )));
                    }
                }
            }

            // Validate transform matrix
            validate_transform_matrix(transformation)?;

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Transform operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Transform {
            entities,
            transformation,
        } = operation
        {
            let mut modified_entities = Vec::new();

            // Convert matrix
            let transform_matrix = array_to_matrix4(transformation);

            // Transform each entity
            for entity_id in entities {
                // Get entity
                let entity = context.get_entity(*entity_id)?;

                // Convert to BRep
                let mut brep = entity_state_to_brep(&entity)?;

                // Apply transformation
                apply_transform_to_brep(&mut brep, &transform_matrix)?;

                // Convert back to entity state
                let transformed_entity = brep_to_entity_state(
                    &brep,
                    *entity_id,
                    entity.entity_type,
                    entity
                        .properties
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                )?;

                // Add transform information to properties
                let mut final_entity = transformed_entity;
                if let Some(obj) = final_entity.properties.as_object_mut() {
                    // Store transform history
                    let transforms = if let Some(existing) = obj.get("transform_history") {
                        if let Some(arr) = existing.as_array() {
                            let mut history = arr.clone();
                            history.push(serde_json::json!(transformation));
                            serde_json::json!(history)
                        } else {
                            serde_json::json!([transformation])
                        }
                    } else {
                        serde_json::json!([transformation])
                    };
                    obj.insert("transform_history".to_string(), transforms);

                    // Store cumulative transform
                    let cumulative = if let Some(existing) = obj.get("cumulative_transform") {
                        if let Some(arr) = existing.as_array() {
                            if arr.len() == 4 {
                                let existing_matrix = array_from_json(arr)?;
                                let cumulative_matrix =
                                    multiply_matrices(&existing_matrix, transformation);
                                serde_json::json!(cumulative_matrix)
                            } else {
                                serde_json::json!(transformation)
                            }
                        } else {
                            serde_json::json!(transformation)
                        }
                    } else {
                        serde_json::json!(transformation)
                    };
                    obj.insert("cumulative_transform".to_string(), cumulative);
                }

                // Update in context
                context.update_entity(*entity_id, final_entity)?;
                context.increment_geometry_ops();

                // Track modified entity
                modified_entities.push(ModifiedEntity {
                    id: *entity_id,
                    entity_type: entity.entity_type,
                    modifications: vec![Modification::SetProperty(
                        "transform".to_string(),
                        serde_json::json!(transformation),
                    )],
                });
            }

            // Create output with all modified entities
            let outputs = OperationOutputs {
                created: vec![],
                modified: modified_entities.iter().map(|e| e.id).collect(),
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Transform operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Transform { entities, .. } = operation {
            // Estimate based on typical entity complexity
            ResourceEstimate {
                memory_bytes: (10_000 * entities.len()) as u64, // ~10KB per entity
                time_ms: 10 * entities.len() as u64,            // 10ms per entity
                entities_created: 0,
                entities_modified: entities.len(),
            }
        } else {
            ResourceEstimate::default()
        }
    }
}

/// Apply transformation to all vertices in BRep
fn apply_transform_to_brep(brep: &mut BRepModel, transform: &Matrix4) -> TimelineResult<()> {
    // Transform all vertices
    // VertexStore doesn't implement Iterator, iterate by index
    for idx in 0..brep.vertices.len() {
        let vertex_id = idx as u32;
        if let Some(vertex) = brep.vertices.get(vertex_id) {
            let point = vertex.point();
            let transformed = transform.transform_point(&point);
            brep.vertices
                .set_position(vertex_id, transformed.x, transformed.y, transformed.z);
        }
    }

    // Update surface geometry if present
    // FaceStore doesn't implement Iterator, iterate by index
    for idx in 0..brep.faces.len() {
        let face_id = idx as u32;
        if let Some(_face) = brep.faces.get_mut(face_id) {
            // In a full implementation, would transform surface parameters
            // For now, surface references remain unchanged as surfaces are
            // transformed separately
        }
    }

    // Update curve geometry if present
    // EdgeStore doesn't implement Iterator, iterate by index
    for idx in 0..brep.edges.len() {
        let edge_id = idx as u32;
        if let Some(_edge) = brep.edges.get_mut(edge_id) {
            // In a full implementation, would transform curve parameters
            // For now, curve references remain unchanged as curves are
            // transformed separately
        }
    }

    Ok(())
}

/// Convert JSON array to matrix
fn array_from_json(arr: &[serde_json::Value]) -> TimelineResult<[[f64; 4]; 4]> {
    if arr.len() != 4 {
        return Err(TimelineError::ValidationError(
            "Transform matrix must have 4 rows".to_string(),
        ));
    }

    let mut matrix = [[0.0; 4]; 4];

    for (i, row) in arr.iter().enumerate() {
        if let Some(row_arr) = row.as_array() {
            if row_arr.len() != 4 {
                return Err(TimelineError::ValidationError(format!(
                    "Transform matrix row {} must have 4 columns",
                    i
                )));
            }

            for (j, val) in row_arr.iter().enumerate() {
                matrix[i][j] = val.as_f64().ok_or_else(|| {
                    TimelineError::ValidationError(format!(
                        "Transform matrix element [{},{}] must be a number",
                        i, j
                    ))
                })?;
            }
        } else {
            return Err(TimelineError::ValidationError(format!(
                "Transform matrix row {} must be an array",
                i
            )));
        }
    }

    Ok(matrix)
}

/// Multiply two 4x4 matrices
fn multiply_matrices(a: &[[f64; 4]; 4], b: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
    let mut result = [[0.0; 4]; 4];

    for i in 0..4 {
        for j in 0..4 {
            for k in 0..4 {
                result[i][j] += a[i][k] * b[k][j];
            }
        }
    }

    result
}

/// Create translation matrix
pub fn create_translation_matrix(translation: &[f64; 3]) -> [[f64; 4]; 4] {
    [
        [1.0, 0.0, 0.0, translation[0]],
        [0.0, 1.0, 0.0, translation[1]],
        [0.0, 0.0, 1.0, translation[2]],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Create rotation matrix from axis and angle
pub fn create_rotation_matrix(axis: &[f64; 3], angle_degrees: f64) -> [[f64; 4]; 4] {
    let angle = angle_degrees.to_radians();
    let cos_a = angle.cos();
    let sin_a = angle.sin();
    let one_minus_cos = 1.0 - cos_a;

    // Normalize axis
    let magnitude = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let ux = axis[0] / magnitude;
    let uy = axis[1] / magnitude;
    let uz = axis[2] / magnitude;

    [
        [
            cos_a + ux * ux * one_minus_cos,
            ux * uy * one_minus_cos - uz * sin_a,
            ux * uz * one_minus_cos + uy * sin_a,
            0.0,
        ],
        [
            uy * ux * one_minus_cos + uz * sin_a,
            cos_a + uy * uy * one_minus_cos,
            uy * uz * one_minus_cos - ux * sin_a,
            0.0,
        ],
        [
            uz * ux * one_minus_cos - uy * sin_a,
            uz * uy * one_minus_cos + ux * sin_a,
            cos_a + uz * uz * one_minus_cos,
            0.0,
        ],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Create scale matrix
pub fn create_scale_matrix(scale: &[f64; 3]) -> [[f64; 4]; 4] {
    [
        [scale[0], 0.0, 0.0, 0.0],
        [0.0, scale[1], 0.0, 0.0],
        [0.0, 0.0, scale[2], 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Create matrix from quaternion
pub fn create_matrix_from_quaternion(quaternion: &[f64; 4]) -> [[f64; 4]; 4] {
    let w = quaternion[0];
    let x = quaternion[1];
    let y = quaternion[2];
    let z = quaternion[3];

    let xx = x * x;
    let xy = x * y;
    let xz = x * z;
    let xw = x * w;
    let yy = y * y;
    let yz = y * z;
    let yw = y * w;
    let zz = z * z;
    let zw = z * w;

    [
        [1.0 - 2.0 * (yy + zz), 2.0 * (xy - zw), 2.0 * (xz + yw), 0.0],
        [2.0 * (xy + zw), 1.0 - 2.0 * (xx + zz), 2.0 * (yz - xw), 0.0],
        [2.0 * (xz - yw), 2.0 * (yz + xw), 1.0 - 2.0 * (xx + yy), 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::EntityStateStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_transform_validation() {
        let op = TransformOp;
        let store = Arc::new(EntityStateStore::new());
        let mut context = ExecutionContext::new(crate::BranchId::main(), store);

        // Create test solid entity
        let solid_id = EntityId::new();
        let brep = BRepModel::new();
        let entity = brep_to_entity_state(
            &brep,
            solid_id,
            EntityType::Solid,
            Some("Test Solid".to_string()),
        )
        .unwrap();
        context.add_temp_entity(entity).unwrap();

        // Valid transform
        let operation = Operation::Transform {
            entities: vec![solid_id],
            transformation: [
                [1.0, 0.0, 0.0, 10.0],
                [0.0, 1.0, 0.0, 20.0],
                [0.0, 0.0, 1.0, 30.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        assert!(op.validate(&operation, &context).await.is_ok());

        // Invalid - degenerate matrix
        let operation = Operation::Transform {
            entities: vec![solid_id],
            transformation: [
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };
        assert!(op.validate(&operation, &context).await.is_err());

        // Invalid - non-affine matrix
        let operation = Operation::Transform {
            entities: vec![solid_id],
            transformation: [
                [1.0, 0.0, 0.0, 0.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [1.0, 0.0, 0.0, 1.0],
            ],
        };
        assert!(op.validate(&operation, &context).await.is_err());
    }

    #[test]
    fn test_translation_matrix() {
        let matrix = create_translation_matrix(&[10.0, 20.0, 30.0]);
        assert_eq!(matrix[0][3], 10.0);
        assert_eq!(matrix[1][3], 20.0);
        assert_eq!(matrix[2][3], 30.0);
        assert_eq!(matrix[3][3], 1.0);
    }

    #[test]
    fn test_rotation_matrix() {
        // 90 degree rotation around Z axis
        let matrix = create_rotation_matrix(&[0.0, 0.0, 1.0], 90.0);

        // Apply to point (1, 0, 0) should give (0, 1, 0)
        let x = 1.0 * matrix[0][0] + 0.0 * matrix[0][1] + 0.0 * matrix[0][2];
        let y = 1.0 * matrix[1][0] + 0.0 * matrix[1][1] + 0.0 * matrix[1][2];

        assert!((x - 0.0).abs() < 1e-10);
        assert!((y - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_scale_matrix() {
        let matrix = create_scale_matrix(&[2.0, 3.0, 4.0]);
        assert_eq!(matrix[0][0], 2.0);
        assert_eq!(matrix[1][1], 3.0);
        assert_eq!(matrix[2][2], 4.0);
        assert_eq!(matrix[3][3], 1.0);
    }
}
