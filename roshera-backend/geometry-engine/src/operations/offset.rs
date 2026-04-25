//! Offset and Shell Operations for B-Rep Models
//!
//! Creates offset surfaces and shells (hollowed solids) by moving faces
//! normal to their surface by specified distances.
//!
//! # References
//! - Maekawa, T. (1999). An overview of offset curves and surfaces. CAD.
//! - Pham, B. (1992). Offset curves and surfaces: a brief survey. CAD.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex,
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

/// Create offset cylinder by adjusting radius
fn create_offset_cylinder(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Cylinder;

    let cyl = surface
        .as_any()
        .downcast_ref::<Cylinder>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Cylinder surface".into()))?;

    let new_radius = cyl.radius + distance;
    if new_radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Offset produces zero or negative cylinder radius".into(),
        ));
    }

    let mut offset = Cylinder::new(cyl.origin, cyl.axis, new_radius)?;
    offset.height_limits = cyl.height_limits;
    offset.angle_limits = cyl.angle_limits;
    Ok(Box::new(offset))
}

/// Create offset sphere by adjusting radius
fn create_offset_sphere(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Sphere;

    let sph = surface
        .as_any()
        .downcast_ref::<Sphere>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Sphere surface".into()))?;

    let new_radius = sph.radius + distance;
    if new_radius <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Offset produces zero or negative sphere radius".into(),
        ));
    }

    let mut offset = Sphere::new(sph.center, new_radius)?;
    offset.param_limits = sph.param_limits;
    Ok(Box::new(offset))
}

/// Create offset cone — offset along normal moves the surface, keeping half angle constant
fn create_offset_cone(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Cone;

    let cone = surface
        .as_any()
        .downcast_ref::<Cone>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Cone surface".into()))?;

    // Offsetting a cone along its normal shifts the apex along the axis
    // by distance / sin(half_angle), keeping the half angle constant.
    let shift = distance / cone.half_angle.sin();
    let new_apex = cone.apex + cone.axis * shift;

    let mut offset = Cone::new(new_apex, cone.axis, cone.half_angle)?;
    offset.height_limits = cone.height_limits.map(|[lo, hi]| [lo - shift, hi - shift]);
    offset.angle_limits = cone.angle_limits;
    Ok(Box::new(offset))
}

/// Create offset torus by adjusting minor radius
fn create_offset_torus(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::surface::Torus;

    let tor = surface
        .as_any()
        .downcast_ref::<Torus>()
        .ok_or_else(|| OperationError::InvalidGeometry("Expected Torus surface".into()))?;

    let new_minor = tor.minor_radius + distance;
    if new_minor <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Offset produces zero or negative torus minor radius".into(),
        ));
    }

    let mut offset = Torus::new(tor.center, tor.axis, tor.major_radius, new_minor)?;
    offset.param_limits = tor.param_limits;
    Ok(Box::new(offset))
}

/// Create offset B-spline / NURBS surface using the Surface::offset trait method
fn create_offset_bspline(
    surface: &dyn Surface,
    distance: f64,
) -> OperationResult<Box<dyn Surface>> {
    Ok(surface.offset(distance))
}

/// Create offset NURBS surface using the Surface::offset trait method
fn create_offset_nurbs(surface: &dyn Surface, distance: f64) -> OperationResult<Box<dyn Surface>> {
    Ok(surface.offset(distance))
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
    // Validate that the requested offset distance is geometrically meaningful
    // relative to the user-supplied tolerance. A near-zero offset would
    // produce vertices coincident with the source edge, generating a
    // numerical artifact rather than a real offset.
    let tol = options.common.tolerance.distance();
    if !distance.is_finite() {
        return Err(OperationError::InvalidGeometry(format!(
            "create_offset_edge: distance {} is not finite",
            distance
        )));
    }
    if distance.abs() <= tol {
        return Err(OperationError::InvalidGeometry(format!(
            "create_offset_edge: |distance|={:.3e} is not greater than tolerance {:.3e}",
            distance.abs(),
            tol
        )));
    }
    // Reject offsets that exceed the configured deviation budget — those
    // approximations would silently degrade surface quality without warning.
    if options.max_deviation > 0.0 && distance.abs() > options.max_deviation * 1e6 {
        return Err(OperationError::InvalidGeometry(format!(
            "create_offset_edge: distance {:.3e} far exceeds max_deviation {:.3e}",
            distance, options.max_deviation,
        )));
    }
    // Honor the caller's direction preference: forward=false flips the offset
    // sign so callers can request either side of the source curve.
    let signed_distance = if forward { distance } else { -distance };

    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry(format!(
            "create_offset_edge: edge {} not found",
            edge_id
        )))?
        .clone();

    // Get edge curve
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Create offset curve along the requested side (signed distance honors
    // forward=false by flipping the side of the source curve).
    let offset_curve = create_offset_curve(curve, surface_id, signed_distance)?;
    let curve_id = model.curves.add(offset_curve);

    // Create offset vertices
    let start_pos = edge.evaluate(0.0, &model.curves)?;
    let end_pos = edge.evaluate(1.0, &model.curves)?;

    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Offset vertices along surface normal (signed by forward flag)
    let start_normal = compute_surface_normal_at_point(surface, start_pos)?;
    let end_normal = compute_surface_normal_at_point(surface, end_pos)?;

    let offset_start = model.vertices.add(
        start_pos.x + start_normal.x * signed_distance,
        start_pos.y + start_normal.y * signed_distance,
        start_pos.z + start_normal.z * signed_distance,
    );
    let offset_end = model.vertices.add(
        end_pos.x + end_normal.x * signed_distance,
        end_pos.y + end_normal.y * signed_distance,
        end_pos.z + end_normal.z * signed_distance,
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

/// Compute surface normal at a point by finding the closest parametric location
fn compute_surface_normal_at_point(
    surface: &dyn Surface,
    point: Point3,
) -> OperationResult<Vector3> {
    let tol = Tolerance::new(1e-6, 1e-6);
    let (u, v) = surface.closest_point(&point, tol)?;
    Ok(surface.normal_at(u, v)?)
}

/// Extend offset loop corners by intersecting adjacent offset surfaces.
///
/// Not yet implemented. Correct handling requires:
/// 1. Detecting tangent discontinuities between consecutive offset edges.
/// 2. Intersecting the two adjacent offset surfaces (SSI) to obtain the
///    extended corner curve.
/// 3. Rewriting each neighbour edge's parametric range to terminate on the
///    new corner vertex, then inserting that vertex and the corner edge.
///
/// Until that work lands, this function returns `NotImplemented` so callers
/// of `CornerType::Extended` are not silently given a non-watertight result.
fn extend_offset_corners(
    _model: &mut BRepModel,
    _offset_edges: &mut Vec<(EdgeId, bool)>,
) -> OperationResult<()> {
    Err(OperationError::NotImplemented(
        "CornerType::Extended offset corners not implemented; \
         use CornerType::Natural for the current kernel"
            .to_string(),
    ))
}

/// Insert fillet arcs between consecutive offset edges.
///
/// Not yet implemented. Correct handling requires:
/// 1. Detecting tangent discontinuities between consecutive offset edges.
/// 2. Constructing an arc edge of the requested `radius` tangent to both
///    neighbour edges on the offset surface, with centre at the corner
///    normal offset inward.
/// 3. Trimming the two neighbour edges back to the arc tangency points and
///    stitching the new arc edge + two tangency vertices into the loop.
///
/// Until that work lands, this function returns `NotImplemented` so callers
/// of `CornerType::Round` are not silently given a non-watertight result.
fn round_offset_corners(
    _model: &mut BRepModel,
    _offset_edges: &mut Vec<(EdgeId, bool)>,
    _radius: f64,
) -> OperationResult<()> {
    Err(OperationError::NotImplemented(
        "CornerType::Round offset corners not implemented; \
         use CornerType::Natural for the current kernel"
            .to_string(),
    ))
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
    use crate::primitives::curve::Line;
    use crate::primitives::edge::EdgeOrientation;
    use crate::primitives::face::FaceOrientation;
    use crate::primitives::r#loop::LoopType;
    use crate::primitives::surface::Plane;

    let outer_edge = model
        .edges
        .get(outer_edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Outer edge not found".into()))?
        .clone();

    // Get the outer edge endpoints
    let p1_arr = model
        .vertices
        .get_position(outer_edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".into()))?;
    let p2_arr = model
        .vertices
        .get_position(outer_edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".into()))?;
    let p1 = Vector3::new(p1_arr[0], p1_arr[1], p1_arr[2]);
    let p2 = Vector3::new(p2_arr[0], p2_arr[1], p2_arr[2]);

    // Compute offset direction from the surface normal at midpoint
    let edge_curve = model
        .curves
        .get(outer_edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge curve not found".into()))?;
    let mid = edge_curve.evaluate(0.5)?.position;
    let edge_dir = (p2 - p1).normalize()?;
    // Use a default inward direction perpendicular to edge
    let offset_dir = if edge_dir.cross(&Vector3::Z).magnitude_squared() > 1e-6 {
        edge_dir.cross(&Vector3::Z).normalize()?
    } else {
        edge_dir.cross(&Vector3::Y).normalize()?
    };

    let offset = offset_dir * (-thickness.abs());
    let p3 = p2 + offset;
    let p4 = p1 + offset;

    // Create the planar surface through the four corners
    let wall_normal = edge_dir.cross(&offset_dir).normalize()?;
    let wall_surface = Plane::from_point_normal(p1, wall_normal)?;
    let surface_id = model.surfaces.add(Box::new(wall_surface));

    // Create vertices for inner edge
    let v3 = model.vertices.add(p3.x, p3.y, p3.z);
    let v4 = model.vertices.add(p4.x, p4.y, p4.z);

    // Create four edges for the rectangular face
    let e_top = outer_edge_id; // reuse outer edge
    let line_right = Line::new(p2, p3);
    let c_right = model.curves.add(Box::new(line_right));
    let e_right = model.edges.add(Edge::new_auto_range(
        0,
        outer_edge.end_vertex,
        v3,
        c_right,
        EdgeOrientation::Forward,
    ));

    let line_bottom = Line::new(p3, p4);
    let c_bottom = model.curves.add(Box::new(line_bottom));
    let e_bottom = model.edges.add(Edge::new_auto_range(
        0,
        v3,
        v4,
        c_bottom,
        EdgeOrientation::Forward,
    ));

    let line_left = Line::new(p4, p1);
    let c_left = model.curves.add(Box::new(line_left));
    let e_left = model.edges.add(Edge::new_auto_range(
        0,
        v4,
        outer_edge.start_vertex,
        c_left,
        EdgeOrientation::Forward,
    ));

    // Create loop
    let mut wall_loop = Loop::new(0, LoopType::Outer);
    wall_loop.add_edge(e_top, forward);
    wall_loop.add_edge(e_right, true);
    wall_loop.add_edge(e_bottom, true);
    wall_loop.add_edge(e_left, true);
    let loop_id = model.loops.add(wall_loop);

    // Create face
    let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
    let face_id = model.faces.add(face);

    Ok(face_id)
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
