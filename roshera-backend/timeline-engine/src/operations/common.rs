//! Common utilities for operation implementations

use super::brep_helpers::BRepModelExt;
use crate::{
    brep_serialization::{deserialize_brep, serialize_brep},
    entity_mapping::get_entity_mapping,
    execution::{EntityState, ExecutionContext},
    EntityId, EntityType, TimelineError, TimelineResult,
};
use geometry_engine::{
    math::{Matrix4, Point3, Vector3},
    primitives::{
        solid::SolidId,
        topology_builder::{BRepModel, GeometryId as GeometryEngineId},
    },
};
use shared_types::GeometryId;

/// Convert geometry engine BRep to entity state
pub fn brep_to_entity_state(
    brep: &BRepModel,
    entity_id: EntityId,
    entity_type: EntityType,
    name: Option<String>,
) -> TimelineResult<EntityState> {
    // Use our new serialization system
    let geometry_json = serialize_brep(brep)?;
    let geometry_data = geometry_json.into_bytes();

    // Create properties
    let properties = serde_json::json!({
        "name": name,
        "type": format!("{:?}", entity_type),
        "vertex_count": brep.vertices.len(),
        "edge_count": brep.edges.len(),
        "face_count": brep.faces.len(),
        "shell_count": brep.shells.len(),
        "solid_count": brep.solids.len(),
    });

    Ok(EntityState {
        id: entity_id,
        entity_type,
        geometry_data,
        properties,
        is_deleted: false,
    })
}

/// Convert entity state back to BRep model
pub fn entity_state_to_brep(entity: &EntityState) -> TimelineResult<BRepModel> {
    // Use our new deserialization system
    let geometry_json = String::from_utf8(entity.geometry_data.clone()).map_err(|e| {
        TimelineError::DeserializationError(format!("Invalid UTF-8 in geometry data: {}", e))
    })?;
    deserialize_brep(&geometry_json)
}

/// Create a geometry ID from entity ID
pub fn entity_id_to_geometry_id(entity_id: EntityId) -> GeometryId {
    GeometryId(entity_id.0)
}

/// Create an entity ID from geometry ID
pub fn geometry_id_to_entity_id(geometry_id: GeometryId) -> EntityId {
    // GeometryId now contains a UUID directly
    EntityId(geometry_id.0)
}

/// Validate a transformation matrix
pub fn validate_transform_matrix(matrix: &[[f64; 4]; 4]) -> TimelineResult<()> {
    // Check for NaN or infinity
    for row in matrix {
        for value in row {
            if !value.is_finite() {
                return Err(TimelineError::ValidationError(
                    "Transformation matrix contains non-finite values".to_string(),
                ));
            }
        }
    }

    // Check determinant is non-zero (not degenerate)
    let det = calculate_determinant_4x4(matrix);
    if det.abs() < 1e-10 {
        return Err(TimelineError::ValidationError(
            "Transformation matrix is degenerate (determinant near zero)".to_string(),
        ));
    }

    // Check last row is [0, 0, 0, 1] for affine transformation
    if matrix[3][0].abs() > 1e-10
        || matrix[3][1].abs() > 1e-10
        || matrix[3][2].abs() > 1e-10
        || (matrix[3][3] - 1.0).abs() > 1e-10
    {
        return Err(TimelineError::ValidationError(
            "Transformation matrix is not affine (last row should be [0, 0, 0, 1])".to_string(),
        ));
    }

    Ok(())
}

/// Calculate 4x4 matrix determinant
pub fn calculate_determinant_4x4(m: &[[f64; 4]; 4]) -> f64 {
    // Calculate using cofactor expansion along first row
    let m00 = m[0][0];
    let m01 = m[0][1];
    let m02 = m[0][2];
    let m03 = m[0][3];

    // Calculate 3x3 minors
    let minor00 = det3x3(
        m[1][1], m[1][2], m[1][3], m[2][1], m[2][2], m[2][3], m[3][1], m[3][2], m[3][3],
    );

    let minor01 = det3x3(
        m[1][0], m[1][2], m[1][3], m[2][0], m[2][2], m[2][3], m[3][0], m[3][2], m[3][3],
    );

    let minor02 = det3x3(
        m[1][0], m[1][1], m[1][3], m[2][0], m[2][1], m[2][3], m[3][0], m[3][1], m[3][3],
    );

    let minor03 = det3x3(
        m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2], m[3][0], m[3][1], m[3][2],
    );

    m00 * minor00 - m01 * minor01 + m02 * minor02 - m03 * minor03
}

/// Calculate 3x3 determinant
fn det3x3(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64, g: f64, h: f64, i: f64) -> f64 {
    a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g)
}

/// Convert Matrix4 to array format
pub fn matrix4_to_array(matrix: &Matrix4) -> [[f64; 4]; 4] {
    let mut result = [[0.0; 4]; 4];
    for i in 0..4 {
        for j in 0..4 {
            result[i][j] = matrix[(i, j)];
        }
    }
    result
}

/// Convert array to Matrix4
pub fn array_to_matrix4(array: &[[f64; 4]; 4]) -> Matrix4 {
    Matrix4::from_rows_array(*array)
}

/// Extract sketch plane information from properties
pub fn extract_sketch_plane(
    properties: &serde_json::Value,
) -> TimelineResult<(Point3, Vector3, Vector3)> {
    let origin = if let Some(origin_val) = properties.get("origin") {
        if let Some(arr) = origin_val.as_array() {
            if arr.len() == 3 {
                Point3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                )
            } else {
                Point3::new(0.0, 0.0, 0.0)
            }
        } else {
            Point3::new(0.0, 0.0, 0.0)
        }
    } else {
        Point3::new(0.0, 0.0, 0.0)
    };

    let normal = if let Some(normal_val) = properties.get("normal") {
        if let Some(arr) = normal_val.as_array() {
            if arr.len() == 3 {
                let v = Vector3::new(
                    arr[0].as_f64().unwrap_or(0.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(1.0),
                );
                v.normalize().unwrap_or(Vector3::Z)
            } else {
                Vector3::Z
            }
        } else {
            Vector3::Z
        }
    } else {
        Vector3::Z
    };

    let x_dir = if let Some(x_dir_val) = properties.get("x_direction") {
        if let Some(arr) = x_dir_val.as_array() {
            if arr.len() == 3 {
                let v = Vector3::new(
                    arr[0].as_f64().unwrap_or(1.0),
                    arr[1].as_f64().unwrap_or(0.0),
                    arr[2].as_f64().unwrap_or(0.0),
                );
                v.normalize().unwrap_or(Vector3::X)
            } else {
                Vector3::X
            }
        } else {
            Vector3::X
        }
    } else {
        // Calculate x direction perpendicular to normal
        let cross_x = normal.cross(&Vector3::X);
        if cross_x.magnitude() > 0.1 {
            cross_x.normalize().unwrap_or(Vector3::X)
        } else {
            normal.cross(&Vector3::Y).normalize().unwrap_or(Vector3::X)
        }
    };

    Ok((origin, normal, x_dir))
}

/// Validate edges belong to the same solid
pub fn validate_edges_same_solid(
    edges: &[EntityId],
    _context: &ExecutionContext,
) -> TimelineResult<EntityId> {
    if edges.is_empty() {
        return Err(TimelineError::ValidationError(
            "No edges provided".to_string(),
        ));
    }

    // Use the entity mapping to find parent solids
    let mapping = get_entity_mapping();

    // Find the common parent solid for all edges
    let mut parent_solid: Option<SolidId> = None;
    for &edge_id in edges {
        if let Some(solid_id) = mapping.get_parent_solid(edge_id) {
            if let Some(existing_parent) = parent_solid {
                if existing_parent != solid_id {
                    return Err(TimelineError::ValidationError(
                        "Edges belong to different solids".to_string(),
                    ));
                }
            } else {
                parent_solid = Some(solid_id);
            }
        } else {
            return Err(TimelineError::ValidationError(format!(
                "Edge {:?} has no parent solid",
                edge_id
            )));
        }
    }

    // Find the entity ID for this solid
    if let Some(solid_id) = parent_solid {
        if let Some(entity_id) = mapping.get_entity_id(&GeometryEngineId::Solid(solid_id)) {
            return Ok(entity_id);
        }
    }

    // Original implementation commented out due to ID type mismatch
    /*
    for solid_entity in solids {
        let brep = entity_state_to_brep(&solid_entity)?;

        // Check if all edges belong to this solid
        let mut all_edges_found = true;
        for &edge_id in edges {
            let geometry_id = entity_id_to_geometry_id(edge_id);

            // Check if edge exists in any face of any shell of this solid
            let mut edge_found = false;
            for (_solid_id, solid) in &brep.solids {
                // Check outer shell
                let shell_id = solid.outer_shell;
                if shell_id != 0 {
                    if let Some(shell) = brep.shells.get(shell_id) {
                        for &face_id in &shell.faces {
                            if let Some(face) = brep.faces.get(face_id) {
                                // Check outer loop
                                if let Some(loop_) = brep.loops.get(face.outer_loop) {
                                    for &edge_id_in_loop in &loop_.edges {
                                        if edge_id_in_loop == geometry_id.0 {
                                            edge_found = true;
                                            break;
                                        }
                                    }
                                }

                                // Check inner loops
                                if !edge_found {
                                    for &loop_id in &face.inner_loops {
                                    if let Some(loop_) = brep.loops.get(loop_id) {
                                        for &edge_id_in_loop in &loop_.edges {
                                            if edge_id_in_loop == geometry_id.0 {
                                                edge_found = true;
                                                break;
                                            }
                                        }
                                    }
                                    if edge_found { break; }
                                }
                            }
                            if edge_found { break; }
                        }
                    }
                    if edge_found { break; }
                }

                // Also check inner shells
                if !edge_found {
                    for &inner_shell_id in &solid.inner_shells {
                        if let Some(shell) = brep.shells.get(inner_shell_id) {
                            for &face_id in &shell.faces {
                                if let Some(face) = brep.faces.get(face_id) {
                                    // Check outer loop
                                    if let Some(loop_) = brep.loops.get(face.outer_loop) {
                                        for &edge_id_in_loop in &loop_.edges {
                                            if edge_id_in_loop == geometry_id.0 {
                                                edge_found = true;
                                                break;
                                            }
                                        }
                                    }

                                    // Check inner loops
                                    if !edge_found {
                                        for &loop_id in &face.inner_loops {
                                            if let Some(loop_) = brep.loops.get(loop_id) {
                                                for &edge_id_in_loop in &loop_.edges {
                                                    if edge_id_in_loop == geometry_id.0 {
                                                        edge_found = true;
                                                        break;
                                                    }
                                                }
                                            }
                                            if edge_found { break; }
                                        }
                                    }
                                    if edge_found { break; }
                                }
                            }
                            if edge_found { break; }
                        }
                    }
                }

                if edge_found { break; }
            }

            if !edge_found {
                all_edges_found = false;
                break;
            }
        }

        if all_edges_found {
            return Ok(solid_entity.id);
        }
    }
    */

    Err(TimelineError::ValidationError(
        "No solid found containing all edges".to_string(),
    ))
}

/// Create a simple box BRep for testing
pub fn create_test_box() -> BRepModel {
    let mut brep = BRepModel::new();

    // Create 8 vertices for a unit cube
    let v1 = brep.add_vertex(Point3::new(0.0, 0.0, 0.0));
    let v2 = brep.add_vertex(Point3::new(1.0, 0.0, 0.0));
    let v3 = brep.add_vertex(Point3::new(1.0, 1.0, 0.0));
    let v4 = brep.add_vertex(Point3::new(0.0, 1.0, 0.0));
    let v5 = brep.add_vertex(Point3::new(0.0, 0.0, 1.0));
    let v6 = brep.add_vertex(Point3::new(1.0, 0.0, 1.0));
    let v7 = brep.add_vertex(Point3::new(1.0, 1.0, 1.0));
    let v8 = brep.add_vertex(Point3::new(0.0, 1.0, 1.0));

    // Create edges
    let _e1 = brep.add_edge(v1, v2, None);
    let _e2 = brep.add_edge(v2, v3, None);
    let _e3 = brep.add_edge(v3, v4, None);
    let _e4 = brep.add_edge(v4, v1, None);
    let _e5 = brep.add_edge(v5, v6, None);
    let _e6 = brep.add_edge(v6, v7, None);
    let _e7 = brep.add_edge(v7, v8, None);
    let _e8 = brep.add_edge(v8, v5, None);
    let _e9 = brep.add_edge(v1, v5, None);
    let _e10 = brep.add_edge(v2, v6, None);
    let _e11 = brep.add_edge(v3, v7, None);
    let _e12 = brep.add_edge(v4, v8, None);

    // Create faces (simplified - normally would have proper loops)
    let shell_id = brep.add_shell(geometry_engine::primitives::shell::ShellType::Closed);
    let solid_id = brep.add_solid();

    if let Some(solid) = brep.solids.get_mut(solid_id) {
        solid.outer_shell = shell_id;
    }

    brep
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_determinant_calculation() {
        // Identity matrix
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!((calculate_determinant_4x4(&identity) - 1.0).abs() < 1e-10);

        // Scaling matrix
        let scale = [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 3.0, 0.0, 0.0],
            [0.0, 0.0, 4.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!((calculate_determinant_4x4(&scale) - 24.0).abs() < 1e-10);
    }

    #[test]
    fn test_transform_validation() {
        // Valid transform
        let valid = [
            [1.0, 0.0, 0.0, 10.0],
            [0.0, 1.0, 0.0, 20.0],
            [0.0, 0.0, 1.0, 30.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!(validate_transform_matrix(&valid).is_ok());

        // Degenerate transform
        let degenerate = [
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        assert!(validate_transform_matrix(&degenerate).is_err());

        // Non-affine transform
        let non_affine = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0, 1.0], // Wrong last row
        ];
        assert!(validate_transform_matrix(&non_affine).is_err());
    }
}
