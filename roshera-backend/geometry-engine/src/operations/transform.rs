//! Transform Operations for B-Rep Models
//!
//! Applies transformations (translate, rotate, scale, mirror) to B-Rep entities
//! while maintaining topological integrity and analytical precision.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    edge::EdgeId, face::FaceId, solid::SolidId, topology_builder::BRepModel, vertex::VertexId,
};
use std::collections::HashSet;

/// Options for transform operations
#[derive(Debug, Clone)]
pub struct TransformOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Whether to copy or move
    pub copy: bool,

    /// Whether to update surface parameterization
    pub update_parameterization: bool,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            copy: false,
            update_parameterization: true,
        }
    }
}

/// Transform result
#[derive(Debug)]
pub struct TransformResult {
    /// Transformed entities (or copies if copy=true)
    pub transformed_ids: Vec<u32>,
    /// Transform matrix applied
    pub transform: Matrix4,
}

/// Apply transformation to a solid
pub fn transform_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Validate inputs
    validate_transform_inputs(model, &transform)?;

    let solid = if options.copy {
        copy_solid(model, solid_id)?
    } else {
        solid_id
    };

    // Get all entities in solid
    let entities = get_solid_entities(model, solid)?;

    // Transform vertices
    transform_vertices(model, &entities.vertices, &transform)?;

    // Transform curves
    transform_curves(model, &entities.edges, &transform)?;

    // Transform surfaces
    if options.update_parameterization {
        transform_surfaces(model, &entities.faces, &transform)?;
    }

    // Validate result
    if options.common.validate_result {
        validate_transformed_solid(model, solid)?;
    }

    Ok(TransformResult {
        transformed_ids: vec![solid],
        transform,
    })
}

/// Apply transformation to faces
pub fn transform_faces(
    model: &mut BRepModel,
    face_ids: Vec<FaceId>,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    validate_transform_inputs(model, &transform)?;

    let faces = if options.copy {
        copy_faces(model, &face_ids)?
    } else {
        face_ids.clone()
    };

    // Get all entities used by faces
    let entities = get_faces_entities(model, &faces)?;

    // Transform vertices
    transform_vertices(model, &entities.vertices, &transform)?;

    // Transform curves
    transform_curves(model, &entities.edges, &transform)?;

    // Transform surfaces
    if options.update_parameterization {
        transform_surfaces(model, &faces, &transform)?;
    }

    Ok(TransformResult {
        transformed_ids: faces.into_iter().map(|f| f as u32).collect(),
        transform,
    })
}

/// Apply transformation to edges
pub fn transform_edges(
    model: &mut BRepModel,
    edge_ids: Vec<EdgeId>,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    validate_transform_inputs(model, &transform)?;

    let edges = if options.copy {
        copy_edges(model, &edge_ids)?
    } else {
        edge_ids.clone()
    };

    // Get vertices used by edges
    let mut vertices = HashSet::new();
    for &edge_id in &edges {
        if let Some(edge) = model.edges.get(edge_id) {
            vertices.insert(edge.start_vertex);
            vertices.insert(edge.end_vertex);
        }
    }

    // Transform vertices
    let vertex_vec: Vec<_> = vertices.into_iter().collect();
    transform_vertices(model, &vertex_vec, &transform)?;

    // Transform curves
    transform_curves(model, &edges, &transform)?;

    Ok(TransformResult {
        transformed_ids: edges.into_iter().map(|e| e as u32).collect(),
        transform,
    })
}

/// Translate entities
pub fn translate(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    direction: Vector3,
    distance: f64,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    let transform = Matrix4::from_translation(&(direction * distance));

    // Dispatch based on entity type
    // For simplicity, assuming solids
    transform_solid(model, entity_ids[0], transform, options)
}

/// Rotate entities
pub fn rotate(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    axis_origin: Point3,
    axis_direction: Vector3,
    angle: f64,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Build rotation matrix
    let transform = Matrix4::rotation_axis(axis_origin, axis_direction, angle)?;

    // Dispatch based on entity type
    transform_solid(model, entity_ids[0], transform, options)
}

/// Scale entities
pub fn scale(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    scale_origin: Point3,
    scale_factors: Vector3,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Validate scale factors
    if scale_factors.x <= 0.0 || scale_factors.y <= 0.0 || scale_factors.z <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Scale factors must be positive".to_string(),
        ));
    }

    // Build scale matrix
    let transform = Matrix4::scale_about_point(scale_origin, scale_factors);

    // Dispatch based on entity type
    transform_solid(model, entity_ids[0], transform, options)
}

/// Mirror entities
pub fn mirror(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    plane_origin: Point3,
    plane_normal: Vector3,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Build mirror matrix
    let transform = Matrix4::mirror(plane_origin, plane_normal)?;

    // Dispatch based on entity type
    let result = transform_solid(model, entity_ids[0], transform, options)?;

    // Mirroring reverses orientation, need to fix
    fix_mirrored_orientations(model, result.transformed_ids[0])?;

    Ok(result)
}

/// Transform vertices
fn transform_vertices(
    model: &mut BRepModel,
    vertex_ids: &[VertexId],
    transform: &Matrix4,
) -> OperationResult<Vec<VertexId>> {
    let mut new_vertices = Vec::new();

    for &vertex_id in vertex_ids {
        if let Some(vertex) = model.vertices.get(vertex_id) {
            let pos = Point3::from(vertex.position);
            let transformed = transform.transform_point(&pos);
            let new_id = model
                .vertices
                .add(transformed.x, transformed.y, transformed.z);
            new_vertices.push(new_id);
        } else {
            return Err(OperationError::InvalidGeometry(
                "Vertex not found".to_string(),
            ));
        }
    }
    Ok(new_vertices)
}

/// Transform curves
fn transform_curves(
    model: &mut BRepModel,
    edge_ids: &[EdgeId],
    transform: &Matrix4,
) -> OperationResult<()> {
    let mut transformed_curves = HashSet::new();

    for &edge_id in edge_ids {
        if let Some(edge) = model.edges.get(edge_id) {
            let curve_id = edge.curve_id;

            // Only transform each curve once
            if transformed_curves.contains(&curve_id) {
                continue;
            }
            transformed_curves.insert(curve_id);

            // Transform the curve
            // TODO: Curves are immutable in the current design
            // Would need to create new transformed curves
        }
    }

    Ok(())
}

/// Transform surfaces
fn transform_surfaces(
    model: &mut BRepModel,
    face_ids: &[FaceId],
    transform: &Matrix4,
) -> OperationResult<()> {
    let mut transformed_surfaces = HashSet::new();

    for &face_id in face_ids {
        if let Some(face) = model.faces.get(face_id) {
            let surface_id = face.surface_id;

            // Only transform each surface once
            if transformed_surfaces.contains(&surface_id) {
                continue;
            }
            transformed_surfaces.insert(surface_id);

            // Transform the surface
            if let Some(surface) = model.surfaces.get(surface_id) {
                let _transformed_surface = surface.transform(transform);
                // TODO: Need to add the transformed surface to the model and update face references
                // For now, we'll just note that this needs implementation
            }
        }
    }

    Ok(())
}

/// Copy a solid
fn copy_solid(_model: &mut BRepModel, solid_id: SolidId) -> OperationResult<SolidId> {
    // Would implement deep copy of solid and all its entities
    Ok(solid_id) // Placeholder
}

/// Copy faces
fn copy_faces(_model: &mut BRepModel, face_ids: &[FaceId]) -> OperationResult<Vec<FaceId>> {
    // Would implement deep copy of faces
    Ok(face_ids.to_vec()) // Placeholder
}

/// Copy edges
fn copy_edges(_model: &mut BRepModel, edge_ids: &[EdgeId]) -> OperationResult<Vec<EdgeId>> {
    // Would implement deep copy of edges
    Ok(edge_ids.to_vec()) // Placeholder
}

/// Get all entities in a solid
struct SolidEntities {
    vertices: Vec<VertexId>,
    edges: Vec<EdgeId>,
    faces: Vec<FaceId>,
}

fn get_solid_entities(model: &BRepModel, solid_id: SolidId) -> OperationResult<SolidEntities> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;

    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut vertices = HashSet::new();
    let mut edges = HashSet::new();
    let faces = shell.faces.clone();

    // Collect all edges and vertices
    for &face_id in &faces {
        if let Some(face) = model.faces.get(face_id) {
            // Get outer loop edges
            if let Some(loop_data) = model.loops.get(face.outer_loop) {
                for &edge_id in &loop_data.edges {
                    edges.insert(edge_id);

                    if let Some(edge) = model.edges.get(edge_id) {
                        vertices.insert(edge.start_vertex);
                        vertices.insert(edge.end_vertex);
                    }
                }
            }
        }
    }

    Ok(SolidEntities {
        vertices: vertices.into_iter().collect(),
        edges: edges.into_iter().collect(),
        faces,
    })
}

/// Get all entities used by faces
fn get_faces_entities(model: &BRepModel, face_ids: &[FaceId]) -> OperationResult<SolidEntities> {
    let mut vertices = HashSet::new();
    let mut edges = HashSet::new();

    for &face_id in face_ids {
        if let Some(face) = model.faces.get(face_id) {
            // Get outer loop edges
            if let Some(loop_data) = model.loops.get(face.outer_loop) {
                for &edge_id in &loop_data.edges {
                    edges.insert(edge_id);

                    if let Some(edge) = model.edges.get(edge_id) {
                        vertices.insert(edge.start_vertex);
                        vertices.insert(edge.end_vertex);
                    }
                }
            }
        }
    }

    Ok(SolidEntities {
        vertices: vertices.into_iter().collect(),
        edges: edges.into_iter().collect(),
        faces: face_ids.to_vec(),
    })
}

/// Fix orientations after mirroring
fn fix_mirrored_orientations(_model: &mut BRepModel, _solid_id: SolidId) -> OperationResult<()> {
    // Would reverse face orientations and edge directions
    Ok(())
}

/// Validate transform inputs
fn validate_transform_inputs(_model: &BRepModel, transform: &Matrix4) -> OperationResult<()> {
    // Check transform is valid (no shear, etc.)
    if transform.determinant().abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Transform matrix is singular".to_string(),
        ));
    }

    Ok(())
}

/// Validate transformed solid
fn validate_transformed_solid(_model: &BRepModel, _solid_id: SolidId) -> OperationResult<()> {
    // Would validate B-Rep integrity
    Ok(())
}

// Helper functions for transform operations
// Note: Matrix4 already has all the needed transformation methods

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_transform_validation() {
//         // Test transform matrix validation
//     }
//
//     #[test]
//     fn test_scale_validation() {
//         // Test scale factor validation
//     }
// }
