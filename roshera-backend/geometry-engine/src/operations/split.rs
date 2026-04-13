//! Split Operations for B-Rep Models
//!
//! Splits faces, edges, and solids using various splitting tools
//! including planes, surfaces, and curves.

use super::intersect::{intersect_curve_surface, intersect_curves, intersect_surfaces};
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    shell::Shell,
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::{Vertex, VertexId},
};

/// Options for split operations
#[derive(Debug, Clone)]
pub struct SplitOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// What to keep after split
    pub keep: SplitKeep,

    /// Whether to split all or stop at first
    pub split_all: bool,

    /// Gap tolerance for splitting
    pub gap_tolerance: f64,
}

impl Default for SplitOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            keep: SplitKeep::Both,
            split_all: true,
            gap_tolerance: 1e-6,
        }
    }
}

/// What to keep after split operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SplitKeep {
    /// Keep both sides
    Both,
    /// Keep positive side (relative to normal)
    Positive,
    /// Keep negative side
    Negative,
    /// Keep the larger piece
    Larger,
    /// Keep the smaller piece
    Smaller,
}

/// Split a face by a plane
pub fn split_face_by_plane(
    model: &mut BRepModel,
    face_id: FaceId,
    plane_origin: Point3,
    plane_normal: Vector3,
    options: SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_split_face_inputs(model, face_id, &options)?;

    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Find intersection curve between face and plane
    let intersection_curve =
        compute_face_plane_intersection(model, &face, plane_origin, plane_normal)?;

    if intersection_curve.is_empty() {
        // No intersection, return original face
        return Ok(vec![face_id]);
    }

    // Split face along intersection curve
    let split_faces = split_face_along_curve(model, &face, &intersection_curve, &options)?;

    // Filter based on keep option
    let result_faces =
        filter_split_results(model, split_faces, plane_origin, plane_normal, options.keep)?;

    Ok(result_faces)
}

/// Split a face by a surface
pub fn split_face_by_surface(
    model: &mut BRepModel,
    face_id: FaceId,
    splitting_surface_id: u32,
    options: SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_split_face_inputs(model, face_id, &options)?;

    // Find intersection curves between faces
    let intersection = intersect_surfaces(
        model,
        face_id,
        face_id, // Would use splitting surface face
        options.common.tolerance,
    )?;

    match intersection {
        super::intersect::IntersectionResult::Curves(curves) => {
            // Split along intersection curves
            split_face_by_intersection_curves(model, face_id, curves, &options)
        }
        super::intersect::IntersectionResult::Surface(_) => {
            // Surfaces are coincident
            Err(OperationError::InvalidGeometry(
                "Surfaces are coincident".to_string(),
            ))
        }
        _ => {
            // No intersection
            Ok(vec![face_id])
        }
    }
}

/// Split an edge at a parameter
pub fn split_edge_at_parameter(
    model: &mut BRepModel,
    edge_id: EdgeId,
    parameter: f64,
) -> OperationResult<(EdgeId, EdgeId)> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Validate parameter
    if parameter <= 0.0 || parameter >= 1.0 {
        return Err(OperationError::InvalidGeometry(
            "Split parameter must be between 0 and 1".to_string(),
        ));
    }

    // Create split vertex
    let split_point = edge.evaluate(parameter, &model.curves)?;
    let split_vertex = model
        .vertices
        .add(split_point.x, split_point.y, split_point.z);

    // Map to curve parameter
    let curve_param = edge.edge_to_curve_parameter(parameter);

    // Create first edge (start to split)
    let edge1 = Edge::new(
        0, // ID will be assigned by store
        edge.start_vertex,
        split_vertex,
        edge.curve_id,
        edge.orientation,
        crate::primitives::curve::ParameterRange::new(edge.param_range.start, curve_param),
    );
    let edge1_id = model.edges.add(edge1);

    // Create second edge (split to end)
    let edge2 = Edge::new(
        0, // ID will be assigned by store
        split_vertex,
        edge.end_vertex,
        edge.curve_id,
        edge.orientation,
        crate::primitives::curve::ParameterRange::new(curve_param, edge.param_range.end),
    );
    let edge2_id = model.edges.add(edge2);

    Ok((edge1_id, edge2_id))
}

/// Split a solid by a plane
pub fn split_solid_by_plane(
    model: &mut BRepModel,
    solid_id: SolidId,
    plane_origin: Point3,
    plane_normal: Vector3,
    options: SplitOptions,
) -> OperationResult<Vec<SolidId>> {
    // Validate inputs
    validate_split_solid_inputs(model, solid_id, &options)?;

    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    // Split all faces of the solid
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?
        .clone();

    let mut split_face_groups = Vec::new();

    for &face_id in &shell.faces {
        let split_faces =
            split_face_by_plane(model, face_id, plane_origin, plane_normal, options.clone())?;
        split_face_groups.push(split_faces);
    }

    // Create cap faces at split plane
    let cap_faces = create_cap_faces(model, &split_face_groups, plane_origin, plane_normal)?;

    // Assemble split solids
    let split_solids = assemble_split_solids(
        model,
        split_face_groups,
        cap_faces,
        plane_origin,
        plane_normal,
    )?;

    Ok(split_solids)
}

/// Compute face-plane intersection curve
fn compute_face_plane_intersection(
    model: &BRepModel,
    face: &Face,
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<Point3>> {
    // Would compute actual intersection curve
    // For now, return empty (no intersection)
    Ok(Vec::new())
}

/// Split face along curve
fn split_face_along_curve(
    model: &mut BRepModel,
    face: &Face,
    split_curve: &[Point3],
    options: &SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    // Would split face into multiple faces along curve
    // For now, return original face
    Ok(vec![face.id])
}

/// Split face by intersection curves
fn split_face_by_intersection_curves(
    model: &mut BRepModel,
    face_id: FaceId,
    curves: Vec<super::intersect::IntersectionCurve>,
    options: &SplitOptions,
) -> OperationResult<Vec<FaceId>> {
    // Would split face along multiple curves
    Ok(vec![face_id])
}

/// Filter split results based on keep option
fn filter_split_results(
    model: &BRepModel,
    faces: Vec<FaceId>,
    plane_origin: Point3,
    plane_normal: Vector3,
    keep: SplitKeep,
) -> OperationResult<Vec<FaceId>> {
    match keep {
        SplitKeep::Both => Ok(faces),
        SplitKeep::Positive => {
            // Keep faces on positive side of plane
            filter_faces_by_side(model, faces, plane_origin, plane_normal, true)
        }
        SplitKeep::Negative => {
            // Keep faces on negative side of plane
            filter_faces_by_side(model, faces, plane_origin, plane_normal, false)
        }
        SplitKeep::Larger => {
            // Keep larger faces
            filter_faces_by_size(model, faces, true)
        }
        SplitKeep::Smaller => {
            // Keep smaller faces
            filter_faces_by_size(model, faces, false)
        }
    }
}

/// Filter faces by side of plane
fn filter_faces_by_side(
    model: &BRepModel,
    faces: Vec<FaceId>,
    plane_origin: Point3,
    plane_normal: Vector3,
    positive_side: bool,
) -> OperationResult<Vec<FaceId>> {
    let mut result = Vec::new();

    for face_id in faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        // Check which side face centroid is on
        let centroid = compute_face_centroid(model, face)?;
        let to_centroid = centroid - plane_origin;
        let dot = to_centroid.dot(&plane_normal);

        if (dot > 0.0) == positive_side {
            result.push(face_id);
        }
    }

    Ok(result)
}

/// Filter faces by size
fn filter_faces_by_size(
    model: &BRepModel,
    faces: Vec<FaceId>,
    keep_larger: bool,
) -> OperationResult<Vec<FaceId>> {
    // Would compute face areas and filter
    // For now, return all faces
    Ok(faces)
}

/// Compute face centroid
fn compute_face_centroid(model: &BRepModel, face: &Face) -> OperationResult<Point3> {
    // Would compute actual centroid
    // For now, return origin
    Ok(Point3::ZERO)
}

/// Create cap faces at split plane
fn create_cap_faces(
    model: &mut BRepModel,
    split_face_groups: &[Vec<FaceId>],
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<FaceId>> {
    // Would create planar faces to cap the split
    Ok(Vec::new())
}

/// Assemble split solids from faces
fn assemble_split_solids(
    model: &mut BRepModel,
    split_face_groups: Vec<Vec<FaceId>>,
    cap_faces: Vec<FaceId>,
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<SolidId>> {
    // Would assemble faces into separate solids
    Ok(Vec::new())
}

/// Validate split face inputs
fn validate_split_face_inputs(
    model: &BRepModel,
    face_id: FaceId,
    options: &SplitOptions,
) -> OperationResult<()> {
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    Ok(())
}

/// Validate split solid inputs
fn validate_split_solid_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    options: &SplitOptions,
) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_edge_split() {
//         // Test edge splitting at parameter
//     }
// }
