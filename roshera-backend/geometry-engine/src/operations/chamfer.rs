//! Chamfer Operations for B-Rep Models
//!
//! Creates beveled transitions between faces by cutting edges at specified angles or distances.

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

/// Options for chamfer operations
#[derive(Debug, Clone)]
pub struct ChamferOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of chamfer
    pub chamfer_type: ChamferType,

    /// Distance from edge on first face
    pub distance1: f64,

    /// Distance from edge on second face
    pub distance2: f64,

    /// Whether chamfer is symmetric (equal distances)
    pub symmetric: bool,

    /// Propagation mode for edge selection
    pub propagation: PropagationMode,

    /// Whether to preserve original edges in special cases
    pub preserve_edges: bool,
}

impl Default for ChamferOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            chamfer_type: ChamferType::EqualDistance(1.0),
            distance1: 1.0,
            distance2: 1.0,
            symmetric: true,
            propagation: PropagationMode::None,
            preserve_edges: false,
        }
    }
}

/// Type of chamfer
#[derive(Debug, Clone)]
pub enum ChamferType {
    /// Equal distance from edge on both faces
    EqualDistance(f64),
    /// Different distances on each face
    TwoDistances(f64, f64),
    /// Distance and angle
    DistanceAngle(f64, f64),
    /// Symmetric at specified angle (45° default)
    Angle(f64),
}

/// How to propagate chamfer selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationMode {
    /// No propagation
    None,
    /// Propagate along tangent edges
    Tangent,
    /// Propagate along smooth edges
    Smooth,
}

/// Apply chamfer to edges
pub fn chamfer_edges(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: ChamferOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_chamfer_inputs(model, solid_id, &edges, &options)?;

    // Propagate edge selection if requested
    let selected_edges = propagate_edge_selection(model, edges, options.propagation)?;

    // Create chamfer faces for each edge
    let mut chamfer_faces = Vec::new();
    for &edge_id in &selected_edges {
        let face_id = create_edge_chamfer(model, solid_id, edge_id, &options)?;
        chamfer_faces.push(face_id);
    }

    // Update adjacent faces to account for chamfer
    update_adjacent_faces_for_chamfer(model, solid_id, &selected_edges, &chamfer_faces)?;

    // Handle vertex conditions where multiple chamfers meet
    if selected_edges.len() > 1 {
        handle_chamfer_vertices(model, solid_id, &selected_edges, &chamfer_faces)?;
    }

    // Validate result if requested
    if options.common.validate_result {
        validate_chamfered_solid(model, solid_id)?;
    }

    Ok(chamfer_faces)
}

/// Create chamfer for a single edge
fn create_edge_chamfer(
    model: &mut BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
    options: &ChamferOptions,
) -> OperationResult<FaceId> {
    // Get adjacent faces
    let (face1_id, face2_id) = get_adjacent_faces(model, solid_id, edge_id)?;

    // Create chamfer based on type
    match &options.chamfer_type {
        ChamferType::EqualDistance(dist) => {
            create_equal_distance_chamfer(model, edge_id, face1_id, face2_id, *dist)
        }
        ChamferType::TwoDistances(dist1, dist2) => {
            create_two_distance_chamfer(model, edge_id, face1_id, face2_id, *dist1, *dist2)
        }
        ChamferType::DistanceAngle(dist, angle) => {
            create_distance_angle_chamfer(model, edge_id, face1_id, face2_id, *dist, *angle)
        }
        ChamferType::Angle(angle) => {
            create_angle_chamfer(model, edge_id, face1_id, face2_id, *angle)
        }
    }
}

/// Create equal distance chamfer
fn create_equal_distance_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance: f64,
) -> OperationResult<FaceId> {
    // Get edge geometry
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Compute chamfer offsets along edge
    let chamfer_data =
        compute_chamfer_offsets(model, &edge, face1_id, face2_id, distance, distance)?;

    // Create chamfer surface (ruled surface between offset curves)
    let chamfer_surface = create_ruled_chamfer_surface(model, &chamfer_data)?;
    let surface_id = model.surfaces.add(chamfer_surface);

    // Create chamfer face with proper boundaries
    let chamfer_face = create_chamfer_face(model, surface_id, edge_id, &chamfer_data)?;

    Ok(chamfer_face)
}

/// Create two-distance chamfer
fn create_two_distance_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance1: f64,
    distance2: f64,
) -> OperationResult<FaceId> {
    // Similar to equal distance but with different offsets
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    let chamfer_data =
        compute_chamfer_offsets(model, &edge, face1_id, face2_id, distance1, distance2)?;

    let chamfer_surface = create_ruled_chamfer_surface(model, &chamfer_data)?;
    let surface_id = model.surfaces.add(chamfer_surface);

    let chamfer_face = create_chamfer_face(model, surface_id, edge_id, &chamfer_data)?;

    Ok(chamfer_face)
}

/// Create distance-angle chamfer
fn create_distance_angle_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance: f64,
    angle: f64,
) -> OperationResult<FaceId> {
    // Compute second distance from angle
    let face_angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let distance2 = distance * (angle.sin() / (face_angle - angle).sin());

    create_two_distance_chamfer(model, edge_id, face1_id, face2_id, distance, distance2)
}

/// Create angle-based chamfer
fn create_angle_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    angle: f64,
) -> OperationResult<FaceId> {
    // For symmetric angle chamfer, compute equal distances
    let face_angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let half_angle = face_angle / 2.0;

    // Choose a reasonable default distance
    let distance = 1.0; // Would be computed from edge length

    create_equal_distance_chamfer(model, edge_id, face1_id, face2_id, distance)
}

/// Data for chamfer computation
struct ChamferData {
    /// Points on first face offset curve
    offset_points1: Vec<Point3>,
    /// Points on second face offset curve
    offset_points2: Vec<Point3>,
    /// Parameter values along edge
    parameters: Vec<f64>,
    /// Normal directions on faces
    normals1: Vec<Vector3>,
    normals2: Vec<Vector3>,
}

/// Compute chamfer offset curves
fn compute_chamfer_offsets(
    model: &BRepModel,
    edge: &Edge,
    face1_id: FaceId,
    face2_id: FaceId,
    distance1: f64,
    distance2: f64,
) -> OperationResult<ChamferData> {
    let num_samples = 10;
    let mut data = ChamferData {
        offset_points1: Vec::new(),
        offset_points2: Vec::new(),
        parameters: Vec::new(),
        normals1: Vec::new(),
        normals2: Vec::new(),
    };

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        data.parameters.push(t);

        // Get point on edge
        let curve = model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
        let edge_point = curve.point_at(t).map_err(|e| {
            OperationError::NumericalError(format!("Edge evaluation failed: {:?}", e))
        })?;

        // Compute offset directions (simplified)
        // In reality, would compute proper face normals and offset directions
        let offset_dir1 = Vector3::new(1.0, 0.0, 0.0).normalize().map_err(|e| {
            OperationError::NumericalError(format!("Vector normalization failed: {:?}", e))
        })?;
        let offset_dir2 = Vector3::new(0.0, 1.0, 0.0).normalize().map_err(|e| {
            OperationError::NumericalError(format!("Vector normalization failed: {:?}", e))
        })?;

        data.offset_points1
            .push(edge_point + offset_dir1 * distance1);
        data.offset_points2
            .push(edge_point + offset_dir2 * distance2);
        data.normals1.push(offset_dir1);
        data.normals2.push(offset_dir2);
    }

    Ok(data)
}

/// Create ruled surface for chamfer
fn create_ruled_chamfer_surface(
    model: &mut BRepModel,
    data: &ChamferData,
) -> OperationResult<Box<dyn Surface>> {
    // Would create proper ruled surface between offset curves
    use crate::primitives::surface::Plane;
    Ok(Box::new(Plane::xy(0.0)))
}

/// Create chamfer face with boundaries
fn create_chamfer_face(
    model: &mut BRepModel,
    surface_id: u32,
    edge_id: EdgeId,
    data: &ChamferData,
) -> OperationResult<FaceId> {
    // Create edges for chamfer boundary
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Create offset edge curves
    let offset_curve1 = create_offset_curve(model, &data.offset_points1)?;
    let offset_curve2 = create_offset_curve(model, &data.offset_points2)?;

    // Create vertices at ends
    let v1_start = model.vertices.add(
        data.offset_points1[0].x,
        data.offset_points1[0].y,
        data.offset_points1[0].z,
    );
    let v1_end = model.vertices.add(
        data.offset_points1.last().unwrap().x,
        data.offset_points1.last().unwrap().y,
        data.offset_points1.last().unwrap().z,
    );
    let v2_start = model.vertices.add(
        data.offset_points2[0].x,
        data.offset_points2[0].y,
        data.offset_points2[0].z,
    );
    let v2_end = model.vertices.add(
        data.offset_points2.last().unwrap().x,
        data.offset_points2.last().unwrap().y,
        data.offset_points2.last().unwrap().z,
    );

    // Create edges
    let edge1 = Edge::new_auto_range(
        0, // Will be assigned by store
        v1_start,
        v1_end,
        offset_curve1,
        EdgeOrientation::Forward,
    );
    let edge1_id = model.edges.add(edge1);

    let edge2 = create_straight_edge(model, v1_end, v2_end)?;

    let edge3 = Edge::new_auto_range(
        0, // Will be assigned by store
        v2_end,
        v2_start,
        offset_curve2,
        EdgeOrientation::Forward,
    );
    let edge3_id = model.edges.add(edge3);

    let edge4 = create_straight_edge(model, v2_start, v1_start)?;

    // Create loop
    let mut chamfer_loop = Loop::new(
        0, // Will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    chamfer_loop.add_edge(edge1_id, true);
    chamfer_loop.add_edge(edge2, true);
    chamfer_loop.add_edge(edge3_id, false);
    chamfer_loop.add_edge(edge4, true);
    let loop_id = model.loops.add(chamfer_loop);

    // Create face
    let face = Face::new(
        0, // Will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Create offset curve from points
fn create_offset_curve(model: &mut BRepModel, points: &[Point3]) -> OperationResult<u32> {
    // Would create proper B-spline curve through points
    // For now, create line between endpoints
    use crate::primitives::curve::Line;
    let line = Line::new(points[0], *points.last().unwrap());
    Ok(model.curves.add(Box::new(line)))
}

/// Create straight edge between vertices
fn create_straight_edge(
    model: &mut BRepModel,
    start: VertexId,
    end: VertexId,
) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Line;

    let start_vertex = model
        .vertices
        .get(start)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(end)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let line = Line::new(
        Point3::from(start_vertex.position),
        Point3::from(end_vertex.position),
    );
    let curve_id = model.curves.add(Box::new(line));

    let edge = Edge::new_auto_range(
        0, // Will be assigned by store
        start,
        end,
        curve_id,
        EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Update adjacent faces for chamfer
fn update_adjacent_faces_for_chamfer(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    chamfer_faces: &[FaceId],
) -> OperationResult<()> {
    // Would update face boundaries to account for chamfer
    Ok(())
}

/// Handle vertices where multiple chamfers meet
fn handle_chamfer_vertices(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    chamfer_faces: &[FaceId],
) -> OperationResult<()> {
    // Would create proper vertex treatments (3-way corners, etc.)
    Ok(())
}

/// Propagate edge selection
fn propagate_edge_selection(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
    mode: PropagationMode,
) -> OperationResult<Vec<EdgeId>> {
    match mode {
        PropagationMode::None => Ok(initial_edges),
        PropagationMode::Tangent => propagate_tangent_edges(model, initial_edges),
        PropagationMode::Smooth => propagate_smooth_edges(model, initial_edges),
    }
}

/// Propagate along tangent edges
fn propagate_tangent_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    // Would find all tangent-connected edges
    Ok(initial_edges)
}

/// Propagate along smooth edges
fn propagate_smooth_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    // Would find all smoothly-connected edges
    Ok(initial_edges)
}

/// Get adjacent faces for an edge
fn get_adjacent_faces(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    // Would find the two faces sharing this edge
    Err(OperationError::NotImplemented(
        "Adjacent face finding not yet implemented".to_string(),
    ))
}

/// Compute angle between faces at edge
fn compute_face_angle(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<f64> {
    // Would compute actual angle between faces
    Ok(std::f64::consts::PI / 2.0) // 90 degrees placeholder
}

/// Validate chamfer inputs
fn validate_chamfer_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    options: &ChamferOptions,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check edges exist
    for &edge_id in edges {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Edge not found".to_string(),
            ));
        }
    }

    // Validate chamfer parameters
    match &options.chamfer_type {
        ChamferType::EqualDistance(d) => {
            if *d <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distance must be positive".to_string(),
                ));
            }
        }
        ChamferType::TwoDistances(d1, d2) => {
            if *d1 <= 0.0 || *d2 <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distances must be positive".to_string(),
                ));
            }
        }
        ChamferType::DistanceAngle(d, a) => {
            if *d <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distance must be positive".to_string(),
                ));
            }
            if *a <= 0.0 || *a >= std::f64::consts::PI {
                return Err(OperationError::InvalidGeometry(
                    "Angle must be between 0 and π".to_string(),
                ));
            }
        }
        ChamferType::Angle(a) => {
            if *a <= 0.0 || *a >= std::f64::consts::PI {
                return Err(OperationError::InvalidGeometry(
                    "Angle must be between 0 and π".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Validate chamfered solid
fn validate_chamfered_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    // Would perform full validation
    Ok(())
}

/*
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chamfer_validation() {
        // Test parameter validation
    }
}
*/
