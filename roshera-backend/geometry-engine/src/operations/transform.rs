//! Transform Operations for B-Rep Models
//!
//! Applies transformations (translate, rotate, scale, mirror) to B-Rep entities
//! while maintaining topological integrity and analytical precision.
//!
//! Indexed access into matrix rows / point coordinate arrays is the canonical
//! idiom for affine transformation — bounded by 4x4 matrix and 3D vector
//! constants. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

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

    /// Whether to update surface parameterization
    pub update_parameterization: bool,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            update_parameterization: true,
        }
    }
}

/// Transform result
#[derive(Debug)]
pub struct TransformResult {
    /// Transformed entities (transforms apply in place; callers wanting a
    /// duplicate must clone the underlying solid prior to invocation).
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

    let solid = solid_id;

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

    // Record the operation for timeline / event-sourcing consumers.
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("transform_solid")
            .with_parameters(serde_json::json!({
                "solid_id": solid_id,
                "transform": transform,
                "update_parameterization": options.update_parameterization,
            }))
            .with_inputs(vec![solid_id as u64])
            .with_outputs(vec![solid as u64]),
    );

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

    let input_face_ids: Vec<u64> = face_ids.iter().map(|&f| f as u64).collect();

    let faces = face_ids.clone();

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

    let output_face_ids: Vec<u64> = faces.iter().map(|&f| f as u64).collect();
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("transform_faces")
            .with_parameters(serde_json::json!({
                "transform": transform,
                "update_parameterization": options.update_parameterization,
            }))
            .with_inputs(input_face_ids)
            .with_outputs(output_face_ids),
    );

    Ok(TransformResult {
        transformed_ids: faces.into_iter().map(|f| f).collect(),
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

    let input_edge_ids: Vec<u64> = edge_ids.iter().map(|&e| e as u64).collect();

    let edges = edge_ids.clone();

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

    let output_edge_ids: Vec<u64> = edges.iter().map(|&e| e as u64).collect();
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("transform_edges")
            .with_parameters(serde_json::json!({
                "transform": transform,
                "update_parameterization": options.update_parameterization,
            }))
            .with_inputs(input_edge_ids)
            .with_outputs(output_edge_ids),
    );

    Ok(TransformResult {
        transformed_ids: edges.into_iter().map(|e| e).collect(),
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

/// Transform vertices in place.
///
/// Earlier this routine called `model.vertices.add(...)` for each vertex,
/// which appended a *new* vertex with the transformed position while
/// leaving every edge / loop / face still pointing at the original
/// (untransformed) vertices. Net effect: callers like `translate` /
/// `rotate` / `scale` returned `Ok(...)` while the model was visually
/// unchanged. Mutating in place via `VertexStore::set_position` keeps
/// every existing topology reference valid and actually moves the solid.
fn transform_vertices(
    model: &mut BRepModel,
    vertex_ids: &[VertexId],
    transform: &Matrix4,
) -> OperationResult<Vec<VertexId>> {
    for &vertex_id in vertex_ids {
        let pos = match model.vertices.get(vertex_id) {
            Some(v) => Point3::from(v.position),
            None => {
                return Err(OperationError::InvalidGeometry(
                    "Vertex not found".to_string(),
                ));
            }
        };
        let transformed = transform.transform_point(&pos);
        if !model
            .vertices
            .set_position(vertex_id, transformed.x, transformed.y, transformed.z)
        {
            return Err(OperationError::InvalidGeometry(format!(
                "Failed to update vertex {vertex_id}"
            )));
        }
    }
    Ok(vertex_ids.to_vec())
}

/// Transform curves
fn transform_curves(
    model: &mut BRepModel,
    edge_ids: &[EdgeId],
    transform: &Matrix4,
) -> OperationResult<()> {
    // Collect the set of distinct curve IDs referenced by the edges first, so we
    // do not alias `model.edges` and `model.curves` mutably at the same time.
    let mut curve_ids: Vec<_> = edge_ids
        .iter()
        .filter_map(|&edge_id| model.edges.get(edge_id).map(|e| e.curve_id))
        .collect();
    curve_ids.sort_unstable();
    curve_ids.dedup();

    for curve_id in curve_ids {
        // Swap the curve in-place for its transformed image. Since `Curve::transform`
        // returns a fresh `Box<dyn Curve>`, we can replace the slot directly without
        // invalidating edge references (edges keep pointing to the same CurveId).
        if let Some(slot) = model.curves.get_mut(curve_id) {
            let transformed = slot.transform(transform);
            *slot = transformed;
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
    // Collect the distinct surface IDs up front to avoid holding a reference
    // into `model.faces` while we mutate `model.surfaces`.
    let mut surface_ids: Vec<_> = face_ids
        .iter()
        .filter_map(|&face_id| model.faces.get(face_id).map(|f| f.surface_id))
        .collect();
    surface_ids.sort_unstable();
    surface_ids.dedup();

    for surface_id in surface_ids {
        // Build the transformed surface from the current one and swap it in
        // place so face references stay valid.
        let Some(current) = model.surfaces.get(surface_id) else {
            continue;
        };
        let transformed = current.transform(transform);
        if model.surfaces.replace(surface_id, transformed).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "transform_surfaces: surface {surface_id} not found in store"
            )));
        }
    }

    Ok(())
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

/// Fix face / edge orientations after mirroring.
///
/// A reflection has determinant −1 and reverses the handedness of every
/// loop in the solid. Without flipping orientations, every face's
/// outward normal points inward and the solid is inside-out — booleans,
/// volume integration, and tessellation all silently produce the wrong
/// result. This walks every face in every shell of the solid, flips
/// each face's `FaceOrientation`, and flips every edge orientation
/// inside the face's outer + inner loops so the loop traversal still
/// agrees with the reversed face normal.
fn fix_mirrored_orientations(model: &mut BRepModel, solid_id: SolidId) -> OperationResult<()> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    let shell_ids = solid.all_shells();

    // Collect all face IDs first so we can mutate faces without holding
    // an immutable borrow on shells.
    let mut face_ids: Vec<FaceId> = Vec::new();
    for shell_id in &shell_ids {
        if let Some(shell) = model.shells.get(*shell_id) {
            face_ids.extend(shell.faces.iter().copied());
        }
    }

    // Collect loop IDs per face before mutating.
    let mut face_loops: Vec<(FaceId, Vec<crate::primitives::r#loop::LoopId>)> = Vec::new();
    for &fid in &face_ids {
        if let Some(face) = model.faces.get(fid) {
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            face_loops.push((fid, loops));
        }
    }

    // Flip face orientations.
    for &fid in &face_ids {
        if let Some(face) = model.faces.get_mut(fid) {
            face.orientation = face.orientation.flipped();
        }
    }

    // Flip edge orientations inside each loop. Reverse the edge ordering
    // too so that loop traversal still emits a consistent (head→tail)
    // walk under the new face normal. Loop stores edges and orientations
    // as parallel vectors — both must be reversed in lockstep, then each
    // orientation flag inverted.
    for (_fid, loops) in face_loops {
        for lid in loops {
            if let Some(loop_entity) = model.loops.get_mut(lid) {
                loop_entity.edges.reverse();
                loop_entity.orientations.reverse();
                for o in loop_entity.orientations.iter_mut() {
                    *o = !*o;
                }
            }
        }
    }

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

/// Validate transformed solid by running the full B-Rep validation suite.
fn validate_transformed_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep("Solid not found".to_string()));
    }
    let result = crate::primitives::validation::validate_model_enhanced(
        model,
        crate::math::Tolerance::default(),
        crate::primitives::validation::ValidationLevel::Standard,
    );
    if !result.is_valid {
        let summary = result
            .errors
            .iter()
            .take(3)
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(OperationError::InvalidBRep(format!(
            "Transformed solid failed validation ({} errors): {}",
            result.errors.len(),
            summary
        )));
    }
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
