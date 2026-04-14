//! Offset and Shell Operations for B-Rep Models
//!
//! Creates offset surfaces and shells (hollowed solids) by moving faces
//! normal to their surface by specified distances.
//!
//! # References
//! - Maekawa, T. (1999). An overview of offset curves and surfaces. CAD.
//! - Pham, B. (1992). Offset curves and surfaces: a brief survey. CAD.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::{Vertex, VertexId},
};

/// Options for offset operations
#[derive(Debug)]
pub struct OffsetOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of offset
    pub offset_type: OffsetType,

    /// How to handle self-intersections
    pub intersection_handling: IntersectionHandling,

    /// Whether to extend/trim at sharp corners
    pub corner_type: CornerType,

    /// Maximum deviation for approximations
    pub max_deviation: f64,
}

impl Default for OffsetOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            offset_type: OffsetType::Distance(1.0),
            intersection_handling: IntersectionHandling::Trim,
            corner_type: CornerType::Extended,
            max_deviation: 0.001,
        }
    }
}

/// Type of offset
pub enum OffsetType {
    /// Constant distance offset
    Distance(f64),
    /// Variable distance (function of position)
    Variable(Box<dyn Fn(Point3) -> f64>),
    /// Different distances per face
    PerFace(std::collections::HashMap<FaceId, f64>),
}

impl std::fmt::Debug for OffsetType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OffsetType::Distance(d) => write!(f, "Distance({})", d),
            OffsetType::Variable(_) => write!(f, "Variable(<function>)"),
            OffsetType::PerFace(map) => write!(f, "PerFace({:?})", map),
        }
    }
}

/// How to handle self-intersections in offset
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum IntersectionHandling {
    /// Trim self-intersecting regions
    Trim,
    /// Keep all geometry (may be invalid)
    Keep,
    /// Fail if self-intersection occurs
    Fail,
}

/// How to handle corners in offset
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CornerType {
    /// Extend surfaces at corners
    Extended,
    /// Round corners with arcs
    Round,
    /// Natural intersection
    Natural,
}

/// Offset a single face
pub fn offset_face(
    model: &mut BRepModel,
    face_id: FaceId,
    options: OffsetOptions,
) -> OperationResult<FaceId> {
    // Validate inputs
    validate_offset_face_inputs(model, face_id, &options)?;

    // Get face data
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Get offset distance
    let distance = match &options.offset_type {
        OffsetType::Distance(d) => *d,
        OffsetType::Variable(_) => {
            return Err(OperationError::NotImplemented(
                "Variable offset not yet implemented".to_string(),
            ));
        }
        OffsetType::PerFace(map) => *map.get(&face_id).unwrap_or(&0.0),
    };

    // Create offset surface
    let offset_surface = create_offset_surface(model, &face, distance)?;
    let surface_id = model.surfaces.add(offset_surface);

    // Create offset edges
    let offset_loop = create_offset_loop(model, &face, distance, &options)?;
    let loop_id = model.loops.add(offset_loop);

    // Create new face
    let offset_face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        face.orientation,
    );
    let face_id = model.faces.add(offset_face);

    Ok(face_id)
}

/// Create a shell (hollow) from a solid
pub fn offset_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    thickness: f64,
    faces_to_remove: Vec<FaceId>,
    options: OffsetOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_shell_inputs(model, solid_id, thickness, &faces_to_remove)?;

    // Get solid data
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    // Create offset faces for interior
    let interior_faces =
        create_interior_offset_faces(model, &solid, -thickness.abs(), &faces_to_remove, &options)?;

    // Create side walls for removed faces
    let wall_faces = create_shell_walls(model, &solid, thickness, &faces_to_remove)?;

    // Combine original exterior (minus removed faces) with new interior
    let shell_faces =
        combine_shell_faces(model, &solid, &faces_to_remove, interior_faces, wall_faces)?;

    // Create new shell and solid
    let mut shell = Shell::new(0, ShellType::Closed); // ID will be assigned by store
    for face_id in shell_faces {
        shell.add_face(face_id);
    }
    let shell_id = model.shells.add(shell);

    let hollow_solid = Solid::new(0, shell_id); // ID will be assigned by store
    let hollow_id = model.solids.add(hollow_solid);

    // Validate result if requested
    if options.common.validate_result {
        validate_shell_solid(model, hollow_id)?;
    }

    Ok(hollow_id)
}

/// Create offset surface from original surface
fn create_offset_surface(
    model: &mut BRepModel,
    face: &Face,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Create offset based on surface type
    match surface.surface_type() {
        crate::primitives::surface::SurfaceType::Plane => create_offset_plane(surface, distance),
        crate::primitives::surface::SurfaceType::Cylinder => {
            create_offset_cylinder(surface, distance)
        }
        crate::primitives::surface::SurfaceType::Sphere => create_offset_sphere(surface, distance),
        crate::primitives::surface::SurfaceType::Cone => create_offset_cone(surface, distance),
        crate::primitives::surface::SurfaceType::Torus => create_offset_torus(surface, distance),
        crate::primitives::surface::SurfaceType::BSpline => {
            create_offset_bspline(surface, distance)
        }
        crate::primitives::surface::SurfaceType::NURBS => create_offset_nurbs(surface, distance),
        _ => Err(OperationError::NotImplemented(format!(
            "Offset for surface type {:?} not implemented",
            surface.surface_type()
        ))),
    }
}

/// Create offset plane
fn create_offset_plane(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Plane;

    // Get plane normal and offset along it
    let normal = surface.normal_at(0.5, 0.5)?;
    let point = surface.point_at(0.5, 0.5)?;
    let offset_point = point + normal * distance;

    // Create new plane at offset position
    let offset_plane = Plane::from_point_normal(offset_point, normal)?;
    Ok(Box::new(offset_plane))
}

/// Create offset cylinder — radius changes by `distance` along normal direction
fn create_offset_cylinder(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Cylinder;

    let cyl = surface
        .as_any()
        .downcast_ref::<Cylinder>()
        .ok_or_else(|| OperationError::InternalError("Expected Cylinder surface".to_string()))?;

    let new_radius = cyl.radius + distance;
    if new_radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "Offset distance {} would produce non-positive cylinder radius {}",
            distance, new_radius
        )));
    }

    let mut offset_cyl = Cylinder::new(cyl.origin, cyl.axis, new_radius)
        .map_err(|e| OperationError::NumericalError(format!("Cylinder creation failed: {:?}", e)))?;
    offset_cyl.height_limits = cyl.height_limits;
    offset_cyl.angle_limits = cyl.angle_limits;
    Ok(Box::new(offset_cyl))
}

/// Create offset sphere — radius changes by `distance`
fn create_offset_sphere(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Sphere;

    let sph = surface
        .as_any()
        .downcast_ref::<Sphere>()
        .ok_or_else(|| OperationError::InternalError("Expected Sphere surface".to_string()))?;

    let new_radius = sph.radius + distance;
    if new_radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "Offset distance {} would produce non-positive sphere radius {}",
            distance, new_radius
        )));
    }

    let mut offset_sph = Sphere::new(sph.center, new_radius)
        .map_err(|e| OperationError::NumericalError(format!("Sphere creation failed: {:?}", e)))?;
    offset_sph.param_limits = sph.param_limits;
    Ok(Box::new(offset_sph))
}

/// Create offset cone — the offset of a cone is not a cone (it's an involute surface)
/// so we approximate by moving the apex along the axis and preserving the half-angle.
/// This is exact for the lateral surface normal offset.
fn create_offset_cone(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Cone;

    let cone = surface
        .as_any()
        .downcast_ref::<Cone>()
        .ok_or_else(|| OperationError::InternalError("Expected Cone surface".to_string()))?;

    // The normal on a cone surface makes angle (π/2 - half_angle) with the axis.
    // Offsetting moves the apex by distance / sin(half_angle) along the axis.
    let sin_half = cone.half_angle.sin();
    if sin_half.abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Cone half-angle too small for offset".to_string(),
        ));
    }

    let apex_shift = distance / sin_half;
    let new_apex = cone.apex + cone.axis * apex_shift;

    let mut offset_cone = Cone::new(new_apex, cone.axis, cone.half_angle)
        .map_err(|e| OperationError::NumericalError(format!("Cone creation failed: {:?}", e)))?;
    offset_cone.height_limits = cone.height_limits;
    offset_cone.angle_limits = cone.angle_limits;
    Ok(Box::new(offset_cone))
}

/// Create offset torus — minor radius changes by `distance`
fn create_offset_torus(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Torus;

    let torus = surface
        .as_any()
        .downcast_ref::<Torus>()
        .ok_or_else(|| OperationError::InternalError("Expected Torus surface".to_string()))?;

    let new_minor = torus.minor_radius + distance;
    if new_minor <= 0.0 {
        return Err(OperationError::InvalidGeometry(format!(
            "Offset distance {} would produce non-positive torus minor radius {}",
            distance, new_minor
        )));
    }
    if new_minor >= torus.major_radius {
        return Err(OperationError::InvalidGeometry(format!(
            "Offset torus minor radius {} would exceed major radius {}",
            new_minor, torus.major_radius
        )));
    }

    let mut offset_torus = Torus::new(torus.center, torus.axis, torus.major_radius, new_minor)
        .map_err(|e| OperationError::NumericalError(format!("Torus creation failed: {:?}", e)))?;
    offset_torus.param_limits = torus.param_limits;
    Ok(Box::new(offset_torus))
}

/// Create offset B-spline surface by sampling normals and fitting a new surface
fn create_offset_bspline(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    // Offset of a freeform surface: sample, offset along normals, fit new surface
    create_offset_freeform(surface, distance)
}

/// Create offset NURBS surface by sampling normals and fitting a new surface
fn create_offset_nurbs(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    create_offset_freeform(surface, distance)
}

/// Generic freeform surface offset: sample the surface on a grid,
/// displace each point along its normal by `distance`, then construct
/// a new NURBS surface through the offset points.
fn create_offset_freeform(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    use crate::math::nurbs::NurbsSurface;
    use crate::primitives::surface::GeneralNurbsSurface;

    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let n_u = 20usize;
    let n_v = 20usize;

    let mut offset_points: Vec<Vec<Point3>> = Vec::with_capacity(n_u + 1);

    for i in 0..=n_u {
        let u = u_min + (u_max - u_min) * i as f64 / n_u as f64;
        let mut row = Vec::with_capacity(n_v + 1);
        for j in 0..=n_v {
            let v = v_min + (v_max - v_min) * j as f64 / n_v as f64;
            let pt = surface.point_at(u, v).map_err(|e| {
                OperationError::NumericalError(format!("Surface eval failed: {:?}", e))
            })?;
            let n = surface.normal_at(u, v).map_err(|e| {
                OperationError::NumericalError(format!("Normal eval failed: {:?}", e))
            })?;
            row.push(pt + n * distance);
        }
        offset_points.push(row);
    }

    let degree_u = 3.min(n_u);
    let degree_v = 3.min(n_v);

    // Uniform weights (non-rational)
    let weights: Vec<Vec<f64>> = vec![vec![1.0; n_v + 1]; n_u + 1];

    // Clamped uniform knot vectors
    let knots_u = uniform_knot_vector(n_u + 1, degree_u);
    let knots_v = uniform_knot_vector(n_v + 1, degree_v);

    let nurbs = NurbsSurface::new(
        offset_points,
        weights,
        knots_u,
        knots_v,
        degree_u,
        degree_v,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("NURBS surface creation failed: {}", e))
    })?;

    Ok(Box::new(GeneralNurbsSurface { nurbs }))
}

/// Create a uniform knot vector for a B-spline of given control point count and degree
fn uniform_knot_vector(n_control: usize, degree: usize) -> Vec<f64> {
    let n_knots = n_control + degree + 1;
    let mut knots = Vec::with_capacity(n_knots);

    // Clamped uniform knot vector
    for _ in 0..=degree {
        knots.push(0.0);
    }
    let n_internal = n_knots - 2 * (degree + 1);
    for i in 1..=n_internal {
        knots.push(i as f64 / (n_internal + 1) as f64);
    }
    for _ in 0..=degree {
        knots.push(1.0);
    }

    knots
}

/// Create offset loop (boundary curves)
fn create_offset_loop(
    model: &mut BRepModel,
    face: &Face,
    distance: f64,
    options: &OffsetOptions,
) -> OperationResult<Loop> {
    let original_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut offset_edges = Vec::new();

    // Offset each edge in the loop
    for (i, &edge_id) in original_loop.edges.iter().enumerate() {
        let forward = original_loop.orientations[i];
        let offset_edge_id =
            create_offset_edge(model, edge_id, face.surface_id, distance, forward, options)?;
        offset_edges.push((offset_edge_id, forward));
    }

    // Handle corners based on corner type
    match options.corner_type {
        CornerType::Extended => extend_offset_corners(model, &mut offset_edges)?,
        CornerType::Round => round_offset_corners(model, &mut offset_edges, distance)?,
        CornerType::Natural => {} // Keep natural intersections
    }

    // Create new loop
    let mut offset_loop = Loop::new(
        0, // ID will be assigned by store
        original_loop.loop_type,
    );
    for (edge_id, forward) in offset_edges {
        offset_loop.add_edge(edge_id, forward);
    }

    Ok(offset_loop)
}

/// Create offset edge
fn create_offset_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    surface_id: u32,
    distance: f64,
    forward: bool,
    options: &OffsetOptions,
) -> OperationResult<EdgeId> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Get edge curve
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Create offset curve
    let offset_curve = create_offset_curve(curve, surface_id, distance)?;
    let curve_id = model.curves.add(offset_curve);

    // Create offset vertices
    let start_pos = edge.evaluate(0.0, &model.curves)?;
    let end_pos = edge.evaluate(1.0, &model.curves)?;

    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Offset vertices along surface normal
    let start_normal = compute_surface_normal_at_point(surface, start_pos)?;
    let end_normal = compute_surface_normal_at_point(surface, end_pos)?;

    let offset_start = model.vertices.add(
        start_pos.x + start_normal.x * distance,
        start_pos.y + start_normal.y * distance,
        start_pos.z + start_normal.z * distance,
    );
    let offset_end = model.vertices.add(
        end_pos.x + end_normal.x * distance,
        end_pos.y + end_normal.y * distance,
        end_pos.z + end_normal.z * distance,
    );

    // Create new edge
    let offset_edge = Edge::new(
        0, // ID will be assigned by store
        offset_start,
        offset_end,
        curve_id,
        edge.orientation,
        edge.param_range,
    );
    let offset_edge_id = model.edges.add(offset_edge);

    Ok(offset_edge_id)
}

/// Create offset curve
fn create_offset_curve(
    curve: &dyn Curve,
    surface_id: u32,
    distance: f64,
) -> OperationResult<Box<dyn Curve>> {
    // Would create proper offset curve
    // For now, return copy of original
    Ok(curve.clone_box())
}

/// Compute surface normal at a point
fn compute_surface_normal_at_point(
    surface: &dyn Surface,
    point: Point3,
) -> OperationResult<Vector3> {
    // Would compute actual normal at closest point on surface
    // For now, use parametric center
    Ok(surface.normal_at(0.5, 0.5)?)
}

/// Extend corners in offset loop
fn extend_offset_corners(
    model: &mut BRepModel,
    offset_edges: &mut Vec<(EdgeId, bool)>,
) -> OperationResult<()> {
    // Would extend surfaces at corners to meet
    Ok(())
}

/// Round corners in offset loop
fn round_offset_corners(
    model: &mut BRepModel,
    offset_edges: &mut Vec<(EdgeId, bool)>,
    radius: f64,
) -> OperationResult<()> {
    // Would add arc edges at corners
    Ok(())
}

/// Create interior offset faces for shell
fn create_interior_offset_faces(
    model: &mut BRepModel,
    solid: &Solid,
    thickness: f64,
    faces_to_remove: &[FaceId],
    options: &OffsetOptions,
) -> OperationResult<Vec<FaceId>> {
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?
        .clone();

    let mut interior_faces = Vec::new();

    for &face_id in &shell.faces {
        // Skip faces that will be removed (openings)
        if faces_to_remove.contains(&face_id) {
            continue;
        }

        // Create inward offset of face
        let offset_options = OffsetOptions {
            common: options.common.clone(),
            offset_type: OffsetType::Distance(thickness),
            intersection_handling: options.intersection_handling,
            corner_type: options.corner_type,
            max_deviation: options.max_deviation,
        };

        let interior_face = offset_face(model, face_id, offset_options)?;
        interior_faces.push(interior_face);
    }

    Ok(interior_faces)
}

/// Create wall faces for shell openings
fn create_shell_walls(
    model: &mut BRepModel,
    solid: &Solid,
    thickness: f64,
    faces_to_remove: &[FaceId],
) -> OperationResult<Vec<FaceId>> {
    let mut wall_faces = Vec::new();

    for &face_id in faces_to_remove {
        // Get boundary edges of removed face
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();

        let loop_data = model
            .loops
            .get(face.outer_loop)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
            .clone();

        // Create wall face for each edge
        for (i, &edge_id) in loop_data.edges.iter().enumerate() {
            let forward = loop_data.orientations[i];
            let wall_face = create_wall_face(model, edge_id, thickness, forward)?;
            wall_faces.push(wall_face);
        }
    }

    Ok(wall_faces)
}

/// Create a wall face between outer and inner edges
fn create_wall_face(
    model: &mut BRepModel,
    outer_edge_id: EdgeId,
    thickness: f64,
    forward: bool,
) -> OperationResult<FaceId> {
    // Would create rectangular face connecting outer edge to inner offset edge
    Err(OperationError::NotImplemented(
        "Wall face creation not yet implemented".to_string(),
    ))
}

/// Combine faces for shell solid
fn combine_shell_faces(
    model: &mut BRepModel,
    solid: &Solid,
    faces_to_remove: &[FaceId],
    interior_faces: Vec<FaceId>,
    wall_faces: Vec<FaceId>,
) -> OperationResult<Vec<FaceId>> {
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut all_faces = Vec::new();

    // Add original exterior faces (except removed ones)
    for &face_id in &shell.faces {
        if !faces_to_remove.contains(&face_id) {
            all_faces.push(face_id);
        }
    }

    // Add interior offset faces
    all_faces.extend(interior_faces);

    // Add wall faces
    all_faces.extend(wall_faces);

    Ok(all_faces)
}

/// Validate offset face inputs
fn validate_offset_face_inputs(
    model: &BRepModel,
    face_id: FaceId,
    options: &OffsetOptions,
) -> OperationResult<()> {
    // Check face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    // Check offset distance
    match &options.offset_type {
        OffsetType::Distance(d) => {
            if d.abs() < options.common.tolerance.distance() {
                return Err(OperationError::InvalidGeometry(
                    "Offset distance too small".to_string(),
                ));
            }
        }
        _ => {} // Other types validated during execution
    }

    Ok(())
}

/// Validate shell inputs
fn validate_shell_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    thickness: f64,
    faces_to_remove: &[FaceId],
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check thickness
    if thickness.abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Shell thickness too small".to_string(),
        ));
    }

    // Check faces to remove exist
    for &face_id in faces_to_remove {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Face to remove not found".to_string(),
            ));
        }
    }

    Ok(())
}

/// Validate shell solid
fn validate_shell_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    // Would perform full validation
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep(
            "Shell solid not found".to_string(),
        ));
    }

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_offset_validation() {
//         // Test parameter validation
//     }
// }
