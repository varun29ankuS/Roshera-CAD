//! Extrude operation implementation

use super::brep_helpers::BRepModelExt;
use super::common::{brep_to_entity_state, entity_state_to_brep, extract_sketch_plane};
use crate::{
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    CreatedEntity, EntityId, EntityType, Operation, OperationOutputs,
    TimelineError, TimelineResult,
};
use async_trait::async_trait;
use geometry_engine::{
    math::{Point3, Vector3},
    primitives::{
        r#loop::{LoopId, LoopType},
        shell::ShellType,
        topology_builder::BRepModel,
        vertex::VertexId,
    },
};

/// Implementation of extrude operation
pub struct ExtrudeOp;

#[async_trait]
impl OperationImpl for ExtrudeOp {
    fn operation_type(&self) -> &'static str {
        "extrude"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Extrude {
            sketch_id,
            distance,
            direction,
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

            // Validate distance
            if *distance <= 0.0 {
                return Err(TimelineError::ValidationError(
                    "Extrude distance must be positive".to_string(),
                ));
            }

            // Validate direction if provided
            if let Some(dir) = direction {
                let magnitude = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
                if magnitude < 1e-10 {
                    return Err(TimelineError::ValidationError(
                        "Extrude direction vector must be non-zero".to_string(),
                    ));
                }
            }

            // TODO: Add taper angle validation when it's added to the Operation struct
            // For now, taper angle is not part of the Extrude operation

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Extrude operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Extrude {
            sketch_id,
            distance,
            direction,
        } = operation
        {
            // Get sketch entity
            let sketch_entity = context.get_entity(*sketch_id)?;

            // Convert to BRep
            let sketch_brep = entity_state_to_brep(&sketch_entity)?;

            // Extract sketch plane information
            let (_origin, normal, _x_dir) = extract_sketch_plane(&sketch_entity.properties)?;

            // Determine extrusion direction
            let extrude_dir = if let Some(dir) = direction {
                Vector3::new(dir[0], dir[1], dir[2])
                    .normalize()
                    .unwrap_or(normal)
            } else {
                // Default to sketch normal
                normal
            };

            // Create extruded solid
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
                    "Sketch has no closed loops to extrude".to_string(),
                ));
            }

            // Extrude each loop
            let mut extruded_shells = Vec::new();
            for loop_id in sketch_loops {
                let shell_id = extrude_loop(
                    &sketch_brep,
                    &mut solid_brep,
                    loop_id,
                    &extrude_dir,
                    *distance,
                    0.0, // No taper angle support yet
                )?;
                extruded_shells.push(shell_id);
            }

            // Create a solid from the shells
            let solid_id = solid_brep.add_solid();
            if let Some(solid) = solid_brep.solids.get_mut(solid_id) {
                if !extruded_shells.is_empty() {
                    solid.outer_shell = extruded_shells[0];
                    solid.inner_shells = extruded_shells[1..].to_vec();
                }
            }

            // Create entity for the extruded solid
            let solid_entity_id = EntityId::new();
            let solid_name = format!(
                "Extruded {}",
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

            // Add extrusion-specific properties
            let mut final_entity = entity_state;
            if let Some(obj) = final_entity.properties.as_object_mut() {
                obj.insert("source_sketch".to_string(), serde_json::json!(sketch_id));
                obj.insert("extrude_distance".to_string(), serde_json::json!(distance));
                obj.insert(
                    "extrude_direction".to_string(),
                    serde_json::json!(direction),
                );
                obj.insert("taper_angle".to_string(), serde_json::json!(0.0)); // No taper angle support yet
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
                "Expected Extrude operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Extrude { .. } = operation {
            ResourceEstimate {
                memory_bytes: 50_000, // ~50KB for typical extrusion
                time_ms: 100,         // 100ms typical execution time
                entities_created: 1,
                entities_modified: 0,
            }
        } else {
            ResourceEstimate::default()
        }
    }
}

/// Extrude a loop to create a shell
fn extrude_loop(
    sketch_brep: &BRepModel,
    solid_brep: &mut BRepModel,
    loop_id: LoopId,
    direction: &Vector3,
    distance: f64,
    taper_angle: f64,
) -> TimelineResult<geometry_engine::primitives::shell::ShellId> {
    let loop_ = sketch_brep
        .loops
        .get(loop_id)
        .ok_or_else(|| TimelineError::ExecutionError("Loop not found".to_string()))?;

    // Calculate taper scale factor
    let taper_rad = taper_angle.to_radians();
    let scale_factor = if taper_angle.abs() < 1e-10 {
        1.0
    } else {
        1.0 + distance * taper_rad.tan()
    };

    // Copy vertices from sketch to solid, creating bottom face
    let mut vertex_map = std::collections::HashMap::new();
    let mut bottom_vertices = Vec::new();
    let mut bottom_edges = Vec::new();

    // Create bottom vertices
    for &edge_id in &loop_.edges {
        if let Some(edge) = sketch_brep.edges.get(edge_id) {
            // Copy start vertex if not already copied
            if !vertex_map.contains_key(&edge.start_vertex) {
                if let Some(vertex) = sketch_brep.vertices.get(edge.start_vertex) {
                    let new_vertex = solid_brep.add_vertex(vertex.point());
                    vertex_map.insert(edge.start_vertex, new_vertex);
                    bottom_vertices.push(new_vertex);
                }
            }
        }
    }

    // Create top vertices (extruded)
    let mut top_vertices = Vec::new();
    for &bottom_vertex in &bottom_vertices {
        if let Some(vertex) = solid_brep.vertices.get(bottom_vertex) {
            let offset = *direction * distance;
            let base_pos = vertex.point();

            // Apply taper if needed
            let top_pos = if taper_angle.abs() > 1e-10 {
                // Calculate center point for taper
                let center = calculate_loop_center(solid_brep, &bottom_vertices);
                let to_vertex = Vector3::new(
                    base_pos.x - center.x,
                    base_pos.y - center.y,
                    base_pos.z - center.z,
                );
                Point3::new(
                    center.x + to_vertex.x * scale_factor + offset.x,
                    center.y + to_vertex.y * scale_factor + offset.y,
                    center.z + to_vertex.z * scale_factor + offset.z,
                )
            } else {
                Point3::new(
                    base_pos.x + offset.x,
                    base_pos.y + offset.y,
                    base_pos.z + offset.z,
                )
            };

            let top_vertex = solid_brep.add_vertex(top_pos);
            top_vertices.push(top_vertex);
        }
    }

    // Create bottom edges
    for i in 0..bottom_vertices.len() {
        let v1 = bottom_vertices[i];
        let v2 = bottom_vertices[(i + 1) % bottom_vertices.len()];
        let edge = solid_brep.add_edge(v1, v2, None);
        bottom_edges.push(edge);
    }

    // Create top edges
    let mut top_edges = Vec::new();
    for i in 0..top_vertices.len() {
        let v1 = top_vertices[i];
        let v2 = top_vertices[(i + 1) % top_vertices.len()];
        let edge = solid_brep.add_edge(v1, v2, None);
        top_edges.push(edge);
    }

    // Create vertical edges
    let mut vertical_edges = Vec::new();
    for i in 0..bottom_vertices.len() {
        let bottom_v = bottom_vertices[i];
        let top_v = top_vertices[i];
        let edge = solid_brep.add_edge(bottom_v, top_v, None);
        vertical_edges.push(edge);
    }

    // Create faces
    let shell_id = solid_brep.add_shell(ShellType::Closed);

    // Bottom face
    let bottom_loop_id = solid_brep.add_loop(LoopType::Outer);
    if let Some(bottom_loop) = solid_brep.loops.get_mut(bottom_loop_id) {
        for &edge_id in &bottom_edges {
            bottom_loop.edges.push(edge_id);
            bottom_loop.orientations.push(true);
        }
    }
    let bottom_face_id = solid_brep.add_face(None);
    if let Some(bottom_face) = solid_brep.faces.get_mut(bottom_face_id) {
        bottom_face.outer_loop = bottom_loop_id;
    }

    // Top face
    let top_loop_id = solid_brep.add_loop(LoopType::Outer);
    if let Some(top_loop) = solid_brep.loops.get_mut(top_loop_id) {
        for &edge_id in top_edges.iter().rev() {
            top_loop.edges.push(edge_id);
            top_loop.orientations.push(false);
        }
    }
    let top_face_id = solid_brep.add_face(None);
    if let Some(top_face) = solid_brep.faces.get_mut(top_face_id) {
        top_face.outer_loop = top_loop_id;
    }

    // Side faces
    for i in 0..bottom_vertices.len() {
        let next_i = (i + 1) % bottom_vertices.len();

        // Create loop for side face
        let side_loop_id = solid_brep.add_loop(LoopType::Outer);
        if let Some(side_loop) = solid_brep.loops.get_mut(side_loop_id) {
            side_loop.edges.push(bottom_edges[i]);
            side_loop.orientations.push(true);
            side_loop.edges.push(vertical_edges[next_i]);
            side_loop.orientations.push(true);
            side_loop.edges.push(top_edges[i]);
            side_loop.orientations.push(false);
            side_loop.edges.push(vertical_edges[i]);
            side_loop.orientations.push(false);
        }

        let side_face_id = solid_brep.add_face(None);
        if let Some(side_face) = solid_brep.faces.get_mut(side_face_id) {
            side_face.outer_loop = side_loop_id;
        }

        // Add face to shell
        if let Some(shell) = solid_brep.shells.get_mut(shell_id) {
            shell.faces.push(side_face_id);
        }
    }

    // Add bottom and top faces to shell
    if let Some(shell) = solid_brep.shells.get_mut(shell_id) {
        shell.faces.push(bottom_face_id);
        shell.faces.push(top_face_id);
    }

    Ok(shell_id)
}

/// Calculate the center point of a loop defined by vertices
fn calculate_loop_center(brep: &BRepModel, vertices: &[VertexId]) -> Point3 {
    let mut sum_x = 0.0;
    let mut sum_y = 0.0;
    let mut sum_z = 0.0;
    let mut count = 0;

    for &vertex_id in vertices {
        if let Some(vertex) = brep.vertices.get(vertex_id) {
            let point = vertex.point();
            sum_x += point.x;
            sum_y += point.y;
            sum_z += point.z;
            count += 1;
        }
    }

    if count > 0 {
        Point3::new(
            sum_x / (count as f64),
            sum_y / (count as f64),
            sum_z / (count as f64),
        )
    } else {
        Point3::new(0.0, 0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::EntityStateStore;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_extrude_validation() {
        let op = ExtrudeOp;
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

        // Valid extrude
        let operation = Operation::Extrude {
            sketch_id,
            distance: 10.0,
            direction: None,
        };
        assert!(op.validate(&operation, &context).await.is_ok());

        // Invalid - negative distance
        let operation = Operation::Extrude {
            sketch_id,
            distance: -10.0,
            direction: None,
        };
        assert!(op.validate(&operation, &context).await.is_err());

        // Invalid - sketch doesn't exist
        let operation = Operation::Extrude {
            sketch_id: EntityId::new(),
            distance: 10.0,
            direction: None,
        };
        assert!(op.validate(&operation, &context).await.is_err());
    }
}
