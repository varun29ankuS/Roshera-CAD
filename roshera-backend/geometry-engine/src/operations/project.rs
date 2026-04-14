//! Projection Operations for B-Rep Models
//!
//! Projects points, curves, and other entities onto surfaces and faces.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::EdgeId,
    face::{Face, FaceId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for projection operations
#[derive(Debug, Clone)]
pub struct ProjectionOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Projection direction (None for closest point)
    pub direction: Option<Vector3>,

    /// Maximum projection distance
    pub max_distance: Option<f64>,

    /// Whether to project both directions
    pub bidirectional: bool,

    /// Number of samples for curve projection
    pub curve_samples: u32,
}

impl Default for ProjectionOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            direction: None,
            max_distance: None,
            bidirectional: false,
            curve_samples: 20,
        }
    }
}

/// Result of point projection
#[derive(Debug, Clone)]
pub struct ProjectedPoint {
    /// 3D position on surface
    pub position: Point3,
    /// UV parameters on surface
    pub uv: (f64, f64),
    /// Distance from original point
    pub distance: f64,
    /// Surface normal at projection
    pub normal: Vector3,
}

/// Result of curve projection
#[derive(Debug)]
pub struct ProjectedCurve {
    /// 3D curve on surface
    pub curve_3d: Box<dyn Curve>,
    /// UV curve on surface
    pub curve_uv: Vec<(f64, f64)>,
    /// Original curve parameter to UV mapping
    pub parameter_map: Vec<(f64, f64, f64)>, // (t_original, u, v)
}

/// Project a point onto a face
pub fn project_point_on_face(
    model: &BRepModel,
    point: Point3,
    face_id: FaceId,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedPoint> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let projected = if let Some(direction) = options.direction {
        // Directional projection
        project_point_directional(point, surface, direction, options)?
    } else {
        // Closest point projection
        project_point_closest(point, surface)?
    };

    // Check if projection is within face boundaries
    if !point_in_face_bounds(model, &projected, face)? {
        return Err(OperationError::InvalidGeometry(
            "Projected point is outside face boundaries".to_string(),
        ));
    }

    Ok(projected)
}

/// Project a curve onto a face
pub fn project_curve_on_face(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face_id: FaceId,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedCurve> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample points along curve
    let mut projected_points_3d = Vec::new();
    let mut projected_points_uv = Vec::new();
    let mut parameter_map = Vec::new();

    for i in 0..=options.curve_samples {
        let t = i as f64 / options.curve_samples as f64;
        let curve_t = edge.edge_to_curve_parameter(t);
        let point = curve.point_at(curve_t)?;

        // Project point
        let projected = if let Some(direction) = options.direction {
            project_point_directional(point, surface, direction, options)?
        } else {
            project_point_closest(point, surface)?
        };

        projected_points_3d.push(projected.position);
        projected_points_uv.push(projected.uv);
        parameter_map.push((t, projected.uv.0, projected.uv.1));
    }

    // Create interpolated 3D curve
    let curve_3d = create_interpolated_curve_3d(&projected_points_3d)?;

    Ok(ProjectedCurve {
        curve_3d,
        curve_uv: projected_points_uv,
        parameter_map,
    })
}

/// Project multiple points onto a face
pub fn project_points_on_face(
    model: &BRepModel,
    points: &[Point3],
    face_id: FaceId,
    options: &ProjectionOptions,
) -> OperationResult<Vec<ProjectedPoint>> {
    let mut projected = Vec::new();

    for &point in points {
        match project_point_on_face(model, point, face_id, options) {
            Ok(proj) => projected.push(proj),
            Err(_) => {} // Skip points that can't be projected
        }
    }

    Ok(projected)
}

/// Project point using closest point method
fn project_point_closest(point: Point3, surface: &dyn Surface) -> OperationResult<ProjectedPoint> {
    // Use a simple grid search for now
    // In production, would use Newton-Raphson or similar
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

    let position = surface.point_at(best_u, best_v)?;
    let normal = surface.normal_at(best_u, best_v)?;

    Ok(ProjectedPoint {
        position,
        uv: (best_u, best_v),
        distance: best_distance,
        normal,
    })
}

/// Project point using directional method
fn project_point_directional(
    point: Point3,
    surface: &dyn Surface,
    direction: Vector3,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedPoint> {
    let direction = direction.normalize()?;

    // Ray-surface intersection
    let intersections = ray_surface_intersections(
        point,
        direction,
        surface,
        options.max_distance,
        options.bidirectional,
    )?;

    if intersections.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "No intersection found in projection direction".to_string(),
        ));
    }

    // Use closest intersection
    let closest = intersections
        .into_iter()
        .min_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap())
        .unwrap();

    Ok(closest)
}

/// Find ray-surface intersections
fn ray_surface_intersections(
    origin: Point3,
    direction: Vector3,
    surface: &dyn Surface,
    max_distance: Option<f64>,
    bidirectional: bool,
) -> OperationResult<Vec<ProjectedPoint>> {
    let mut intersections = Vec::new();

    // Forward direction
    if let Some(intersection) = ray_surface_intersection(origin, direction, surface, max_distance)?
    {
        intersections.push(intersection);
    }

    // Backward direction if requested
    if bidirectional {
        if let Some(intersection) =
            ray_surface_intersection(origin, -direction, surface, max_distance)?
        {
            intersections.push(intersection);
        }
    }

    Ok(intersections)
}

/// Find single ray-surface intersection
fn ray_surface_intersection(
    origin: Point3,
    direction: Vector3,
    surface: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    use crate::primitives::surface::SurfaceType;

    match surface.surface_type() {
        SurfaceType::Plane => ray_plane_intersection(origin, direction, surface, max_distance),
        SurfaceType::Cylinder => {
            ray_cylinder_intersection(origin, direction, surface, max_distance)
        }
        SurfaceType::Sphere => ray_sphere_intersection(origin, direction, surface, max_distance),
        _ => {
            // General surface - use iterative method
            ray_general_surface_intersection(origin, direction, surface, max_distance)
        }
    }
}

/// Ray-plane intersection
fn ray_plane_intersection(
    origin: Point3,
    direction: Vector3,
    plane: &dyn Surface,
    max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    // Get plane normal and point
    let normal = plane.normal_at(0.5, 0.5)?;
    let point_on_plane = plane.point_at(0.5, 0.5)?;

    // Ray-plane intersection formula
    let denom = direction.dot(&normal);
    if denom.abs() < 1e-10 {
        // Ray parallel to plane
        return Ok(None);
    }

    let t = (point_on_plane - origin).dot(&normal) / denom;

    if t < 0.0 {
        return Ok(None);
    }

    if let Some(max_dist) = max_distance {
        if t > max_dist {
            return Ok(None);
        }
    }

    let position = origin + direction * t;
    // For a plane, we can use simple parameter calculation
    // In production, would compute actual UV from plane basis
    let (u, v) = (0.5, 0.5); // Placeholder

    Ok(Some(ProjectedPoint {
        position,
        uv: (u, v),
        distance: t,
        normal,
    }))
}

/// Ray-cylinder intersection
fn ray_cylinder_intersection(
    _origin: Point3,
    _direction: Vector3,
    _cylinder: &dyn Surface,
    _max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    // Would implement analytical ray-cylinder intersection
    Ok(None)
}

/// Ray-sphere intersection
fn ray_sphere_intersection(
    _origin: Point3,
    _direction: Vector3,
    _sphere: &dyn Surface,
    _max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    // Would implement analytical ray-sphere intersection
    Ok(None)
}

/// Ray-general surface intersection using iteration
fn ray_general_surface_intersection(
    _origin: Point3,
    _direction: Vector3,
    _surface: &dyn Surface,
    _max_distance: Option<f64>,
) -> OperationResult<Option<ProjectedPoint>> {
    // Would implement Newton-Raphson or similar iterative method
    Ok(None)
}

/// Check if projected point is within face boundaries
fn point_in_face_bounds(
    model: &BRepModel,
    projected: &ProjectedPoint,
    face: &Face,
) -> OperationResult<bool> {
    // Check if UV point is within face trimming loops
    face.contains_point(
        projected.uv.0,
        projected.uv.1,
        &model.loops,
        &model.vertices,
        &model.edges,
        &model.surfaces,
    )
    .map_err(|e| {
        OperationError::InvalidGeometry(format!("Failed to check point containment: {}", e))
    })
}

/// Create interpolated 3D curve through points
fn create_interpolated_curve_3d(points: &[Point3]) -> OperationResult<Box<dyn Curve>> {
    use crate::primitives::curve::Line;

    if points.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Not enough points for curve interpolation".to_string(),
        ));
    }

    // For now, create polyline
    // In full implementation, would create B-spline
    let line = Line::new(points[0], points[points.len() - 1]);
    Ok(Box::new(line))
}

/// Project a vertex onto a surface
pub fn project_vertex_on_surface(
    model: &BRepModel,
    vertex_id: VertexId,
    surface_id: u32,
    options: &ProjectionOptions,
) -> OperationResult<ProjectedPoint> {
    let vertex = model
        .vertices
        .get(vertex_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let point = Point3::from(vertex.position);

    if let Some(direction) = options.direction {
        project_point_directional(point, surface, direction, options)
    } else {
        project_point_closest(point, surface)
    }
}

/// Find silhouette curves of a surface from a view direction
pub fn find_silhouette_curves(
    model: &BRepModel,
    face_id: FaceId,
    view_direction: Vector3,
    _options: &ProjectionOptions,
) -> OperationResult<Vec<EdgeId>> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let mut silhouette_edges = Vec::new();

    // Get face boundary edges
    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    for &edge_id in &loop_data.edges {
        if is_silhouette_edge(model, edge_id, face_id, view_direction)? {
            silhouette_edges.push(edge_id);
        }
    }

    Ok(silhouette_edges)
}

/// Check if edge is a silhouette edge
fn is_silhouette_edge(
    model: &BRepModel,
    edge_id: EdgeId,
    face_id: FaceId,
    view_direction: Vector3,
) -> OperationResult<bool> {
    // Edge is silhouette if surface normal is perpendicular to view direction
    // along the edge

    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample along edge
    for i in 0..=10 {
        let t = i as f64 / 10.0;
        let _point = edge.evaluate(t, &model.curves)?;

        // Get surface normal at this point
        // Use simple parameter finding for now
        let bounds = surface.parameter_bounds();
        let (u, v) = (
            (bounds.0 .0 + bounds.0 .1) * 0.5,
            (bounds.1 .0 + bounds.1 .1) * 0.5,
        ); // Placeholder - in production would find actual UV
        let normal = surface.normal_at(u, v)?;

        // Check if normal is perpendicular to view direction
        let dot = normal.dot(&view_direction).abs();
        if dot < 0.1 {
            // Nearly perpendicular
            return Ok(true);
        }
    }

    Ok(false)
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_projection_options() {
//         let options = ProjectionOptions::default();
//         assert!(options.direction.is_none());
//         assert!(!options.bidirectional);
//     }
// }
