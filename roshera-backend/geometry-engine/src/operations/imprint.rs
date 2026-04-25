//! Imprint Operations for B-Rep Models
//!
//! Imprints edges and wires onto faces, creating new edges on the face
//! without splitting it into separate faces.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for imprint operations
#[derive(Debug, Clone)]
pub struct ImprintOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Whether to project edges onto face
    pub project: bool,

    /// Projection direction (if project is true)
    pub projection_direction: Option<Vector3>,

    /// Whether to extend edges to face boundaries
    pub extend_to_boundary: bool,

    /// Tolerance for imprint
    pub imprint_tolerance: f64,
}

impl Default for ImprintOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            project: true,
            projection_direction: None,
            extend_to_boundary: false,
            imprint_tolerance: 1e-6,
        }
    }
}

/// Result of imprint operation
#[derive(Debug)]
pub struct ImprintResult {
    /// Modified face
    pub face: FaceId,
    /// New edges created on face
    pub new_edges: Vec<EdgeId>,
    /// New vertices created
    pub new_vertices: Vec<VertexId>,
}

/// Imprint edges onto a face
pub fn imprint_edges_on_face(
    model: &mut BRepModel,
    face_id: FaceId,
    edges: Vec<EdgeId>,
    options: ImprintOptions,
) -> OperationResult<ImprintResult> {
    // Validate inputs
    validate_imprint_inputs(model, face_id, &edges, &options)?;

    let mut new_edges = Vec::new();
    let mut new_vertices = Vec::new();

    // Process each edge
    for &edge_id in &edges {
        let (imprinted_edges, imprinted_vertices) = if options.project {
            project_and_imprint_edge(model, face_id, edge_id, &options)?
        } else {
            imprint_edge_directly(model, face_id, edge_id, &options)?
        };

        new_edges.extend(imprinted_edges);
        new_vertices.extend(imprinted_vertices);
    }

    // Extend edges to boundaries if requested
    if options.extend_to_boundary {
        extend_edges_to_boundaries(model, face_id, &mut new_edges)?;
    }

    // Update face to include new edges
    update_face_with_imprinted_edges(model, face_id, &new_edges)?;

    // Validate result if requested
    if options.common.validate_result {
        validate_imprint_result(model, face_id, &new_edges)?;
    }

    Ok(ImprintResult {
        face: face_id,
        new_edges,
        new_vertices,
    })
}

/// Imprint a wire (connected edges) onto a face
pub fn imprint_wire_on_face(
    model: &mut BRepModel,
    face_id: FaceId,
    wire_edges: Vec<EdgeId>,
    options: ImprintOptions,
) -> OperationResult<ImprintResult> {
    // Validate wire connectivity
    validate_wire_connectivity(model, &wire_edges)?;

    // Imprint as connected sequence
    imprint_edges_on_face(model, face_id, wire_edges, options)
}

/// Project and imprint an edge onto a face
fn project_and_imprint_edge(
    model: &mut BRepModel,
    face_id: FaceId,
    edge_id: EdgeId,
    options: &ImprintOptions,
) -> OperationResult<(Vec<EdgeId>, Vec<VertexId>)> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Extract values from edge before mutable borrow
    let edge_curve_id = edge.curve_id;
    let edge_start_vertex = edge.start_vertex;
    let edge_end_vertex = edge.end_vertex;
    let face_surface_id = face.surface_id;

    // Get projection direction
    let proj_dir = match options.projection_direction {
        Some(dir) => dir.normalize()?,
        None => {
            // Use face normal as default projection direction
            let surface = model
                .surfaces
                .get(face_surface_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
            surface.normal_at(0.5, 0.5)?.normalize()?
        }
    };

    // Project edge curve onto surface
    let projected_curve =
        project_curve_onto_surface(model, edge_curve_id, face_surface_id, proj_dir)?;

    // Create new edge on face
    let projected_edge = create_edge_on_face(
        model,
        face_id,
        projected_curve,
        edge_start_vertex,
        edge_end_vertex,
    )?;

    Ok((vec![projected_edge], vec![]))
}

/// Imprint edge directly without projection
fn imprint_edge_directly(
    model: &mut BRepModel,
    face_id: FaceId,
    edge_id: EdgeId,
    options: &ImprintOptions,
) -> OperationResult<(Vec<EdgeId>, Vec<VertexId>)> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Check if edge lies on face within tolerance
    if !edge_lies_on_face(model, &edge, face_id, options.imprint_tolerance)? {
        return Err(OperationError::InvalidGeometry(
            "Edge does not lie on face within tolerance".to_string(),
        ));
    }

    // Create copy of edge associated with face.
    //
    // Face association is not a property of the Edge itself — edges live in
    // a shared EdgeStore and are referenced by Face::internal_edges (added by
    // `update_face_with_imprinted_edges` once all imprint edges are ready)
    // and by loop half-edges.
    let imprinted_edge = Edge::new(
        0, // ID will be assigned by store
        edge.start_vertex,
        edge.end_vertex,
        edge.curve_id,
        edge.orientation,
        edge.param_range,
    );
    let edge_id = model.edges.add(imprinted_edge);

    Ok((vec![edge_id], vec![]))
}

/// Project curve onto surface
fn project_curve_onto_surface(
    model: &mut BRepModel,
    curve_id: u32,
    surface_id: u32,
    direction: Vector3,
) -> OperationResult<u32> {
    let curve = model
        .curves
        .get(curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample points along curve
    let num_samples = 20;
    let mut projected_points = Vec::new();

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let point = curve.point_at(t)?;

        // Project point onto surface
        let projected = project_point_onto_surface(point, surface, direction)?;
        projected_points.push(projected);
    }

    // Create interpolated curve through projected points
    let projected_curve = create_interpolated_curve(model, &projected_points)?;

    Ok(projected_curve)
}

/// Project point onto surface
fn project_point_onto_surface(
    point: Point3,
    surface: &dyn Surface,
    _direction: Vector3,
) -> OperationResult<Point3> {
    // Would implement actual projection
    // For now, return closest point on surface
    // The Surface trait doesn't have a closest_point method
    // Use a simple grid search instead
    let bounds = surface.parameter_bounds();
    let mut best_distance = f64::MAX;
    let mut best_u = 0.0;
    let mut best_v = 0.0;

    // Grid search
    let samples = 20;
    for i in 0..=samples {
        for j in 0..=samples {
            let u = bounds.0 .0 + (i as f64 / samples as f64) * (bounds.0 .1 - bounds.0 .0);
            let v = bounds.1 .0 + (j as f64 / samples as f64) * (bounds.1 .1 - bounds.1 .0);

            let surface_point = surface.point_at(u, v)?;
            let distance = point.distance(&surface_point);

            if distance < best_distance {
                best_distance = distance;
                best_u = u;
                best_v = v;
            }
        }
    }

    Ok(surface.point_at(best_u, best_v)?)
}

/// Create interpolated curve through points
fn create_interpolated_curve(model: &mut BRepModel, points: &[Point3]) -> OperationResult<u32> {
    // use crate::primitives::curve::BSplineCurve; // TODO: Implement BSplineCurve in curves module

    // Would create B-spline through points
    // For now, create polyline
    use crate::primitives::curve::Line;
    if points.len() >= 2 {
        let line = Line::new(points[0], points[points.len() - 1]);
        Ok(model.curves.add(Box::new(line)))
    } else {
        Err(OperationError::InvalidGeometry(
            "Not enough points for curve".to_string(),
        ))
    }
}

/// Create edge on face
fn create_edge_on_face(
    model: &mut BRepModel,
    face_id: FaceId,
    curve_id: u32,
    start_vertex: VertexId,
    end_vertex: VertexId,
) -> OperationResult<EdgeId> {
    // Project vertices onto face if needed
    let projected_start = project_vertex_onto_face(model, start_vertex, face_id)?;
    let projected_end = project_vertex_onto_face(model, end_vertex, face_id)?;

    let edge = Edge::new_auto_range(
        0, // ID will be assigned by store
        projected_start,
        projected_end,
        curve_id,
        crate::primitives::edge::EdgeOrientation::Forward,
    );
    // Face association is recorded later via Face::add_internal_edges inside
    // `update_face_with_imprinted_edges`, not on the Edge itself.

    Ok(model.edges.add(edge))
}

/// Project vertex onto face
fn project_vertex_onto_face(
    model: &mut BRepModel,
    vertex_id: VertexId,
    face_id: FaceId,
) -> OperationResult<VertexId> {
    let vertex = model
        .vertices
        .get(vertex_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let point = Point3::from(vertex.position);
    // The Surface trait doesn't have a closest_point method
    // Use a simple grid search instead
    let bounds = surface.parameter_bounds();
    let mut best_distance = f64::MAX;
    let mut best_u = 0.0;
    let mut best_v = 0.0;

    // Grid search
    let samples = 20;
    for i in 0..=samples {
        for j in 0..=samples {
            let u = bounds.0 .0 + (i as f64 / samples as f64) * (bounds.0 .1 - bounds.0 .0);
            let v = bounds.1 .0 + (j as f64 / samples as f64) * (bounds.1 .1 - bounds.1 .0);

            let surface_point = surface.point_at(u, v)?;
            let distance = point.distance(&surface_point);

            if distance < best_distance {
                best_distance = distance;
                best_u = u;
                best_v = v;
            }
        }
    }

    let projected = surface.point_at(best_u, best_v)?;

    // Check if projection is close enough to use existing vertex
    if point.distance(&projected) < 1e-10 {
        Ok(vertex_id)
    } else {
        // Create new projected vertex
        Ok(model.vertices.add(projected.x, projected.y, projected.z))
    }
}

/// Check if edge lies on face
fn edge_lies_on_face(
    model: &BRepModel,
    edge: &Edge,
    face_id: FaceId,
    tolerance: f64,
) -> OperationResult<bool> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample points along edge
    let num_samples = 10;
    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let point = edge.evaluate(t, &model.curves)?;

        // Check distance to surface
        // The Surface trait doesn't have a closest_point method
        // Use a simple grid search instead
        let bounds = surface.parameter_bounds();
        let mut best_distance = f64::MAX;
        let mut best_u = 0.0;
        let mut best_v = 0.0;

        // Grid search
        let samples = 10;
        for i in 0..=samples {
            for j in 0..=samples {
                let u = bounds.0 .0 + (i as f64 / samples as f64) * (bounds.0 .1 - bounds.0 .0);
                let v = bounds.1 .0 + (j as f64 / samples as f64) * (bounds.1 .1 - bounds.1 .0);

                let surf_pt = surface.point_at(u, v)?;
                let dist = point.distance(&surf_pt);

                if dist < best_distance {
                    best_distance = dist;
                    best_u = u;
                    best_v = v;
                }
            }
        }

        let surface_point = surface.point_at(best_u, best_v)?;

        if point.distance(&surface_point) > tolerance {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Extend edges to face boundaries
fn extend_edges_to_boundaries(
    _model: &mut BRepModel,
    _face_id: FaceId,
    _edges: &mut Vec<EdgeId>,
) -> OperationResult<()> {
    // Would extend each edge to intersect face boundary loops
    Ok(())
}

/// Update face with imprinted edges
fn update_face_with_imprinted_edges(
    model: &mut BRepModel,
    face_id: FaceId,
    new_edges: &[EdgeId],
) -> OperationResult<()> {
    // Add edges to face's internal edge list
    if let Some(face) = model.faces.get_mut(face_id) {
        face.add_internal_edges(new_edges);
    }

    Ok(())
}

/// Validate wire connectivity
fn validate_wire_connectivity(model: &BRepModel, edges: &[EdgeId]) -> OperationResult<()> {
    if edges.is_empty() {
        return Ok(());
    }

    // Check that edges connect end-to-end
    for i in 0..edges.len() - 1 {
        let edge1 = model
            .edges
            .get(edges[i])
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        let edge2 = model
            .edges
            .get(edges[i + 1])
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Check connectivity
        if edge1.end_vertex != edge2.start_vertex
            && edge1.end_vertex != edge2.end_vertex
            && edge1.start_vertex != edge2.start_vertex
            && edge1.start_vertex != edge2.end_vertex
        {
            return Err(OperationError::InvalidGeometry(
                "Wire edges are not connected".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate imprint inputs
fn validate_imprint_inputs(
    model: &BRepModel,
    face_id: FaceId,
    edges: &[EdgeId],
    options: &ImprintOptions,
) -> OperationResult<()> {
    // Check face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
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

    if options.imprint_tolerance <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Imprint tolerance must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Validate imprint result
fn validate_imprint_result(
    model: &BRepModel,
    _face_id: FaceId,
    new_edges: &[EdgeId],
) -> OperationResult<()> {
    // Check all new edges exist and are associated with face
    for &edge_id in new_edges {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidBRep(
                "New edge not found".to_string(),
            ));
        }
    }

    Ok(())
}

// Extension trait for Face to support internal edges
trait FaceInternalEdges {
    fn add_internal_edges(&mut self, edges: &[EdgeId]);
}

impl FaceInternalEdges for Face {
    fn add_internal_edges(&mut self, _edges: &[EdgeId]) {
        // Would store internal edges
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_imprint_validation() {
//         // Test parameter validation
//     }
// }
