//! Revolve operation implementation

use super::brep_helpers::BRepModelExt;
use super::common::{brep_to_entity_state, entity_state_to_brep};
use crate::{
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationOutputs, TimelineError,
    TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::{Matrix4, Point3, Vector3},
    primitives::{
        r#loop::{LoopId, LoopType},
        shell::ShellType,
        topology_builder::BRepModel,
        vertex::VertexId,
    },
};

/// Implementation of revolve operation
pub struct RevolveOp;

#[async_trait]
impl OperationImpl for RevolveOp {
    fn operation_type(&self) -> &'static str {
        "revolve"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Revolve {
            sketch_id,
            axis,
            angle,
        } = operation
        {
            // Validate sketch exists and is a sketch
            let sketch = context.get_entity(*sketch_id)?;

            if sketch.entity_type != EntityType::Sketch {
                return Err(TimelineError::ValidationError(format!(
                    "Entity {} is not a sketch",
                    sketch_id
                )));
            }

            // Validate axis direction
            let axis_magnitude = (axis.direction[0] * axis.direction[0]
                + axis.direction[1] * axis.direction[1]
                + axis.direction[2] * axis.direction[2])
                .sqrt();
            if axis_magnitude < 1e-10 {
                return Err(TimelineError::ValidationError(
                    "Revolve axis direction must be non-zero".to_string(),
                ));
            }

            // Validate angle
            if angle.abs() < 1e-10 {
                return Err(TimelineError::ValidationError(
                    "Revolve angle must be non-zero".to_string(),
                ));
            }

            if angle.abs() > 360.0 {
                return Err(TimelineError::ValidationError(
                    "Revolve angle must be between -360 and 360 degrees".to_string(),
                ));
            }

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Revolve operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Revolve {
            sketch_id,
            axis,
            angle,
        } = operation
        {
            // Get sketch entity
            let sketch_entity = context.get_entity(*sketch_id)?;

            // Convert to BRep
            let sketch_brep = entity_state_to_brep(&sketch_entity)?;

            // Create axis
            let origin = Point3::new(axis.origin[0], axis.origin[1], axis.origin[2]);
            let direction = Vector3::new(axis.direction[0], axis.direction[1], axis.direction[2])
                .normalize()
                .unwrap_or(Vector3::Z);

            // Create revolved solid
            let mut solid_brep = BRepModel::new();

            // Find all loops in the sketch
            let mut sketch_loops = Vec::new();
            // LoopStore doesn't implement Iterator, so we need to iterate by ID
            for loop_id in 0..sketch_brep.loops.len() {
                if let Some(_loop) = sketch_brep.loops.get(loop_id as u32) {
                    sketch_loops.push(loop_id as u32);
                }
            }

            if sketch_loops.is_empty() {
                return Err(TimelineError::ExecutionError(
                    "Sketch has no closed loops to revolve".to_string(),
                ));
            }

            // Check if this is a full revolution
            let is_full_revolution = (angle.abs() - 360.0).abs() < 1e-10;

            // Revolve each loop
            let mut revolved_shells = Vec::new();
            for loop_id in sketch_loops {
                let shell_id = revolve_loop(
                    &sketch_brep,
                    &mut solid_brep,
                    loop_id,
                    &origin,
                    &direction,
                    angle.to_radians(),
                    is_full_revolution,
                )?;
                revolved_shells.push(shell_id);
            }

            // Create a solid from the shells
            let solid_id = solid_brep.add_solid();
            if let Some(solid) = solid_brep.solids.get_mut(solid_id) {
                if !revolved_shells.is_empty() {
                    solid.outer_shell = revolved_shells[0];
                    solid.inner_shells = revolved_shells[1..].to_vec();
                }
            }

            // Create entity for the revolved solid
            let solid_entity_id = EntityId::new();
            let solid_name = format!(
                "Revolved {}",
                sketch_entity
                    .properties
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Sketch")
            );

            let entity_state = brep_to_entity_state(
                &solid_brep,
                solid_entity_id,
                EntityType::Solid,
                Some(solid_name.clone()),
            )?;

            // Add revolve-specific properties
            let mut final_entity = entity_state;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                obj.insert("source_sketch".to_string(), serde_json::json!(sketch_id));
                obj.insert(
                    "revolve_axis_origin".to_string(),
                    serde_json::json!(axis.origin),
                );
                obj.insert(
                    "revolve_axis_direction".to_string(),
                    serde_json::json!(axis.direction),
                );
                obj.insert("revolve_angle".to_string(), serde_json::json!(angle));
            }

            // Add to context
            context.add_temp_entity(final_entity)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![CreatedEntity {
                    id: solid_entity_id,
                    entity_type: EntityType::Solid,
                    name: Some(solid_name),
                }],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Revolve operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Revolve { angle, .. } = operation {
            // More segments needed for larger angles
            let segments = ((angle.abs() / 10.0).ceil() as u64).max(12);

            ResourceEstimate {
                memory_bytes: segments * 5000, // ~5KB per segment
                time_ms: segments * 10,        // ~10ms per segment
                entities_created: 1,
                entities_modified: 0,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}

/// Revolve a loop around an axis to create a shell
fn revolve_loop(
    sketch_brep: &BRepModel,
    solid_brep: &mut BRepModel,
    loop_id: LoopId,
    axis_origin: &Point3,
    axis_direction: &Vector3,
    angle_rad: f64,
    is_full_revolution: bool,
) -> TimelineResult<geometry_engine::primitives::shell::ShellId> {
    let loop_ = sketch_brep
        .loops
        .get(loop_id)
        .ok_or_else(|| TimelineError::ExecutionError("Loop not found".to_string()))?;

    // Calculate number of segments based on angle
    let segments = if is_full_revolution {
        16 // Fixed segments for full revolution
    } else {
        ((angle_rad.abs() / (std::f64::consts::PI / 8.0)).ceil() as usize).max(4)
    };

    let segment_angle = angle_rad / (segments as f64);

    // Copy vertices from sketch to solid, creating profile
    let mut profile_vertices = Vec::new();
    for &edge_id in &loop_.edges {
        if let Some(edge) = sketch_brep.edges.get(edge_id) {
            if let Some(vertex) = sketch_brep.vertices.get(edge.start_vertex) {
                profile_vertices.push(vertex.position);
            }
        }
    }

    if profile_vertices.is_empty() {
        return Err(TimelineError::ExecutionError(
            "No vertices in loop".to_string(),
        ));
    }

    // Create vertices for each revolution segment
    let mut vertex_rings = Vec::new();

    for i in 0..=segments {
        let angle = segment_angle * (i as f64);
        let rotation_matrix = create_rotation_matrix(axis_origin, axis_direction, angle);

        let mut ring = Vec::new();
        for &profile_point in &profile_vertices {
            let point_3d = Point3::new(profile_point[0], profile_point[1], profile_point[2]);
            let rotated_point = rotation_matrix.transform_point(&point_3d);
            let vertex_id = solid_brep.add_vertex(rotated_point);
            ring.push(vertex_id);
        }
        vertex_rings.push(ring);
    }

    // Create shell
    let shell_id = solid_brep.add_shell(ShellType::Closed);

    // Create faces between adjacent rings
    for i in 0..segments {
        let ring1 = &vertex_rings[i];
        let ring2 = &vertex_rings[i + 1];

        // Create faces between rings
        for j in 0..profile_vertices.len() {
            let next_j = (j + 1) % profile_vertices.len();

            // Four vertices of the face
            let v1 = ring1[j];
            let v2 = ring1[next_j];
            let v3 = ring2[next_j];
            let v4 = ring2[j];

            // Create edges
            let e1 = solid_brep.add_edge(v1, v2, None);
            let e2 = solid_brep.add_edge(v2, v3, None);
            let e3 = solid_brep.add_edge(v3, v4, None);
            let e4 = solid_brep.add_edge(v4, v1, None);

            // Create loop
            let face_loop = solid_brep.add_loop(LoopType::Outer);
            if let Some(loop_) = solid_brep.loops.get_mut(face_loop) {
                loop_.edges.push(e1);
                loop_.edges.push(e2);
                loop_.edges.push(e3);
                loop_.edges.push(e4);
                loop_.orientations.push(true);
                loop_.orientations.push(true);
                loop_.orientations.push(true);
                loop_.orientations.push(true);
            }

            // Create face
            let face_id = solid_brep.add_face(None);
            if let Some(face) = solid_brep.faces.get_mut(face_id) {
                face.outer_loop = face_loop;
            }

            // Add face to shell
            if let Some(shell) = solid_brep.shells.get_mut(shell_id) {
                shell.faces.push(face_id);
            }
        }
    }

    // If not a full revolution, create end caps
    if !is_full_revolution {
        // Start cap
        create_end_cap(solid_brep, &vertex_rings[0], &shell_id, false)?;

        // End cap
        create_end_cap(solid_brep, &vertex_rings[segments], &shell_id, true)?;
    }

    Ok(shell_id)
}

/// Create rotation matrix around arbitrary axis
fn create_rotation_matrix(origin: &Point3, axis: &Vector3, angle: f64) -> Matrix4 {
    let cos_angle = angle.cos();
    let sin_angle = angle.sin();
    let one_minus_cos = 1.0 - cos_angle;

    let ux = axis.x;
    let uy = axis.y;
    let uz = axis.z;

    // Rodrigues' rotation formula as a matrix
    let rotation = Matrix4::new(
        cos_angle + ux * ux * one_minus_cos,
        ux * uy * one_minus_cos - uz * sin_angle,
        ux * uz * one_minus_cos + uy * sin_angle,
        0.0,
        uy * ux * one_minus_cos + uz * sin_angle,
        cos_angle + uy * uy * one_minus_cos,
        uy * uz * one_minus_cos - ux * sin_angle,
        0.0,
        uz * ux * one_minus_cos - uy * sin_angle,
        uz * uy * one_minus_cos + ux * sin_angle,
        cos_angle + uz * uz * one_minus_cos,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0,
    );

    // Translate to origin, rotate, translate back
    let translate_to_origin =
        Matrix4::from_translation(&Vector3::new(-origin.x, -origin.y, -origin.z));
    let translate_back = Matrix4::from_translation(&Vector3::new(origin.x, origin.y, origin.z));

    translate_back * rotation * translate_to_origin
}

/// Create end cap for non-full revolution
fn create_end_cap(
    brep: &mut BRepModel,
    vertices: &[VertexId],
    shell_id: &geometry_engine::primitives::shell::ShellId,
    reverse: bool,
) -> TimelineResult<()> {
    if vertices.len() < 3 {
        return Ok(()); // Can't create a face with less than 3 vertices
    }

    // Create edges for the cap
    let mut cap_edges = Vec::new();
    for i in 0..vertices.len() {
        let v1 = if reverse {
            vertices[(i + 1) % vertices.len()]
        } else {
            vertices[i]
        };
        let v2 = if reverse {
            vertices[i]
        } else {
            vertices[(i + 1) % vertices.len()]
        };

        let edge = brep.add_edge(v1, v2, None);
        cap_edges.push(edge);
    }

    // Create loop
    let cap_loop = brep.add_loop(LoopType::Outer);
    if let Some(loop_) = brep.loops.get_mut(cap_loop) {
        for edge in cap_edges {
            loop_.edges.push(edge);
            loop_.orientations.push(true);
        }
    }

    // Create face
    let cap_face = brep.add_face(None);
    if let Some(face) = brep.faces.get_mut(cap_face) {
        face.outer_loop = cap_loop;
    }

    // Add to shell
    if let Some(shell) = brep.shells.get_mut(*shell_id) {
        shell.faces.push(cap_face);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::EntityStateStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_revolve_validation() {
        let op = RevolveOp;
        let store = Arc::new(EntityStateStore::new());
        let mut context = ExecutionContext::new(crate::BranchId::main(), store);

        // Create a test sketch entity
        let sketch_id = EntityId::new();
        let sketch_brep = BRepModel::new();
        let sketch_entity = brep_to_entity_state(
            &sketch_brep,
            sketch_id,
            EntityType::Sketch,
            Some("Test Sketch".to_string()),
        )
        .unwrap();
        context.add_temp_entity(sketch_entity).unwrap();

        // Valid revolve
        let operation = Operation::Revolve {
            sketch_id,
            axis: crate::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            angle: 90.0,
        };
        assert!(op.validate(&operation, &context).await.is_ok());

        // Invalid - zero angle
        let operation = Operation::Revolve {
            sketch_id,
            axis: crate::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            angle: 0.0,
        };
        assert!(op.validate(&operation, &context).await.is_err());

        // Invalid - angle too large
        let operation = Operation::Revolve {
            sketch_id,
            axis: crate::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 1.0],
            },
            angle: 400.0,
        };
        assert!(op.validate(&operation, &context).await.is_err());

        // Invalid - zero axis direction
        let operation = Operation::Revolve {
            sketch_id,
            axis: crate::Axis {
                origin: [0.0, 0.0, 0.0],
                direction: [0.0, 0.0, 0.0],
            },
            angle: 90.0,
        };
        assert!(op.validate(&operation, &context).await.is_err());
    }

    #[test]
    fn test_rotation_matrix() {
        // Test 90 degree rotation around Z axis
        let origin = Point3::new(0.0, 0.0, 0.0);
        let axis = Vector3::Z;
        let matrix = create_rotation_matrix(&origin, &axis, std::f64::consts::PI / 2.0);

        // Point on X axis should rotate to Y axis
        let point = Point3::new(1.0, 0.0, 0.0);
        let rotated = matrix.transform_point(&point);

        assert!((rotated.x - 0.0).abs() < 1e-10);
        assert!((rotated.y - 1.0).abs() < 1e-10);
        assert!((rotated.z - 0.0).abs() < 1e-10);
    }
}
