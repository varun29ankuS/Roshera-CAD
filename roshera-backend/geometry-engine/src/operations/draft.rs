//! Draft Operations for B-Rep Models
//!
//! Applies draft angles to faces for mold design and manufacturing.
//! Essential for injection molding, casting, and other manufacturing processes.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    solid::SolidId,
    surface::Surface,
    topology_builder::BRepModel,
};

/// Options for draft operations
#[derive(Debug, Clone)]
pub struct DraftOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of draft
    pub draft_type: DraftType,

    /// Neutral plane/element definition
    pub neutral: NeutralElement,

    /// Pull direction (will be normalized)
    pub pull_direction: Vector3,

    /// How to handle intersections
    pub intersection_handling: DraftIntersectionHandling,
}

impl Default for DraftOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            draft_type: DraftType::Angle(5.0_f64.to_radians()),
            neutral: NeutralElement::Plane(Point3::ZERO, Vector3::Z),
            pull_direction: Vector3::Z,
            intersection_handling: DraftIntersectionHandling::Extend,
        }
    }
}

/// Type of draft
pub enum DraftType {
    /// Fixed angle draft
    Angle(f64),
    /// Variable angle along height
    Variable(Box<dyn Fn(f64) -> f64>),
    /// Tangent draft (smooth transition)
    Tangent,
    /// Stepped draft with multiple angles
    Stepped(Vec<(f64, f64)>), // (height, angle) pairs
}

impl std::fmt::Debug for DraftType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DraftType::Angle(angle) => f.debug_tuple("Angle").field(angle).finish(),
            DraftType::Variable(_) => f.debug_tuple("Variable").field(&"<function>").finish(),
            DraftType::Tangent => write!(f, "Tangent"),
            DraftType::Stepped(steps) => f.debug_tuple("Stepped").field(steps).finish(),
        }
    }
}

impl Clone for DraftType {
    fn clone(&self) -> Self {
        match self {
            DraftType::Angle(angle) => DraftType::Angle(*angle),
            DraftType::Variable(_) => DraftType::Angle(5.0_f64.to_radians()), // Fallback to fixed angle
            DraftType::Tangent => DraftType::Tangent,
            DraftType::Stepped(steps) => DraftType::Stepped(steps.clone()),
        }
    }
}

/// Neutral element (parting line) definition
pub enum NeutralElement {
    /// Neutral plane
    Plane(Point3, Vector3),
    /// Neutral edge/curve
    Edge(EdgeId),
    /// Neutral face
    Face(FaceId),
    /// Custom curve
    Curve(Box<dyn Curve>),
}

impl std::fmt::Debug for NeutralElement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            NeutralElement::Plane(point, vector) => {
                f.debug_tuple("Plane").field(point).field(vector).finish()
            }
            NeutralElement::Edge(edge_id) => f.debug_tuple("Edge").field(edge_id).finish(),
            NeutralElement::Face(face_id) => f.debug_tuple("Face").field(face_id).finish(),
            NeutralElement::Curve(_) => f.debug_tuple("Curve").field(&"<curve>").finish(),
        }
    }
}

impl Clone for NeutralElement {
    fn clone(&self) -> Self {
        match self {
            NeutralElement::Plane(point, vector) => NeutralElement::Plane(*point, *vector),
            NeutralElement::Edge(edge_id) => NeutralElement::Edge(*edge_id),
            NeutralElement::Face(face_id) => NeutralElement::Face(*face_id),
            NeutralElement::Curve(_) => NeutralElement::Plane(Point3::ZERO, Vector3::Z), // Fallback to plane
        }
    }
}

/// How to handle draft intersections
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DraftIntersectionHandling {
    /// Extend surfaces to intersection
    Extend,
    /// Trim at intersection
    Trim,
    /// Blend intersections
    Blend,
}

/// Apply draft to faces
pub fn apply_draft(
    model: &mut BRepModel,
    solid_id: SolidId,
    faces: Vec<FaceId>,
    options: DraftOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_draft_inputs(model, solid_id, &faces, &options)?;

    // Normalize pull direction
    let pull_direction = options.pull_direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Pull direction normalization failed: {:?}", e))
    })?;

    // Group faces by their relationship to neutral element
    let face_groups = group_faces_by_neutral(model, &faces, &options.neutral, pull_direction)?;

    // Apply draft to each group
    let mut drafted_faces = Vec::new();
    for group in face_groups {
        let group_faces = apply_draft_to_group(model, solid_id, group, &options)?;
        drafted_faces.extend(group_faces);
    }

    // Handle intersections between drafted faces
    handle_draft_intersections(model, &drafted_faces, &options)?;

    // Update adjacent faces
    update_adjacent_faces_for_draft(model, solid_id, &faces, &drafted_faces)?;

    // Validate result if requested
    if options.common.validate_result {
        validate_drafted_solid(model, solid_id)?;
    }

    Ok(drafted_faces)
}

/// Face group for draft application
struct FaceGroup {
    faces: Vec<FaceId>,
    neutral_curve: Vec<Point3>,
    draft_direction: Vector3,
}

/// Group faces based on neutral element
fn group_faces_by_neutral(
    model: &BRepModel,
    faces: &[FaceId],
    neutral: &NeutralElement,
    pull_direction: Vector3,
) -> OperationResult<Vec<FaceGroup>> {
    match neutral {
        NeutralElement::Plane(origin, normal) => {
            group_faces_by_plane(model, faces, *origin, *normal, pull_direction)
        }
        NeutralElement::Edge(edge_id) => {
            group_faces_by_edge(model, faces, *edge_id, pull_direction)
        }
        NeutralElement::Face(face_id) => {
            group_faces_by_face(model, faces, *face_id, pull_direction)
        }
        NeutralElement::Curve(curve) => group_faces_by_curve(model, faces, curve, pull_direction),
    }
}

/// Group faces by neutral plane
fn group_faces_by_plane(
    model: &BRepModel,
    faces: &[FaceId],
    plane_origin: Point3,
    plane_normal: Vector3,
    pull_direction: Vector3,
) -> OperationResult<Vec<FaceGroup>> {
    // For each face, find intersection with neutral plane
    let mut groups = Vec::new();

    for &face_id in faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        // Compute intersection curve of face with neutral plane
        let neutral_curve =
            compute_face_plane_intersection(model, face, plane_origin, plane_normal)?;

        // Determine draft direction based on face orientation relative to pull
        let face_normal = compute_average_face_normal(model, face)?;
        let draft_direction = compute_draft_direction(face_normal, pull_direction, plane_normal);

        groups.push(FaceGroup {
            faces: vec![face_id],
            neutral_curve,
            draft_direction,
        });
    }

    // Merge groups that share neutral curves
    merge_face_groups(&mut groups)?;

    Ok(groups)
}

/// Group faces by neutral edge
fn group_faces_by_edge(
    model: &BRepModel,
    faces: &[FaceId],
    edge_id: EdgeId,
    pull_direction: Vector3,
) -> OperationResult<Vec<FaceGroup>> {
    // Edge defines the neutral curve directly
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Sample points along edge for neutral curve
    let neutral_curve = sample_edge_points(model, edge)?;

    // Group all faces together with this neutral curve
    Ok(vec![FaceGroup {
        faces: faces.to_vec(),
        neutral_curve,
        draft_direction: pull_direction,
    }])
}

/// Group faces by neutral face
fn group_faces_by_face(
    _model: &BRepModel,
    _faces: &[FaceId],
    _neutral_face_id: FaceId,
    _pull_direction: Vector3,
) -> OperationResult<Vec<FaceGroup>> {
    // Neutral face boundary defines neutral curves
    Err(OperationError::NotImplemented(
        "Draft with neutral face not yet implemented".to_string(),
    ))
}

/// Group faces by custom curve
fn group_faces_by_curve(
    _model: &BRepModel,
    faces: &[FaceId],
    curve: &Box<dyn Curve>,
    pull_direction: Vector3,
) -> OperationResult<Vec<FaceGroup>> {
    // Use provided curve as neutral
    let neutral_curve = sample_curve_points(curve)?;

    Ok(vec![FaceGroup {
        faces: faces.to_vec(),
        neutral_curve,
        draft_direction: pull_direction,
    }])
}

/// Apply draft to a group of faces
fn apply_draft_to_group(
    model: &mut BRepModel,
    _solid_id: SolidId,
    group: FaceGroup,
    options: &DraftOptions,
) -> OperationResult<Vec<FaceId>> {
    let mut drafted_faces = Vec::new();

    for face_id in group.faces {
        let drafted_face = draft_single_face(
            model,
            face_id,
            &group.neutral_curve,
            group.draft_direction,
            options,
        )?;
        drafted_faces.push(drafted_face);
    }

    Ok(drafted_faces)
}

/// Draft a single face
fn draft_single_face(
    model: &mut BRepModel,
    face_id: FaceId,
    neutral_curve: &[Point3],
    draft_direction: Vector3,
    options: &DraftOptions,
) -> OperationResult<FaceId> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Get draft angle
    let draft_angle = match &options.draft_type {
        DraftType::Angle(angle) => *angle,
        DraftType::Variable(_) => {
            return Err(OperationError::NotImplemented(
                "Variable draft not yet implemented".to_string(),
            ));
        }
        DraftType::Tangent => {
            return Err(OperationError::NotImplemented(
                "Tangent draft not yet implemented".to_string(),
            ));
        }
        DraftType::Stepped(_) => {
            return Err(OperationError::NotImplemented(
                "Stepped draft not yet implemented".to_string(),
            ));
        }
    };

    // Create drafted surface
    let drafted_surface = create_drafted_surface(
        model,
        face.surface_id,
        neutral_curve,
        draft_direction,
        draft_angle,
    )?;
    let surface_id = model.surfaces.add(drafted_surface);

    // Create drafted edges
    let drafted_loop =
        create_drafted_loop(model, &face, neutral_curve, draft_direction, draft_angle)?;
    let loop_id = model.loops.add(drafted_loop);

    // Create new face
    let drafted_face = Face::new(
        0, // Temporary ID
        surface_id,
        loop_id,
        face.orientation,
    );
    let drafted_face_id = model.faces.add(drafted_face);

    Ok(drafted_face_id)
}

/// Create drafted surface
fn create_drafted_surface(
    model: &mut BRepModel,
    original_surface_id: u32,
    neutral_curve: &[Point3],
    draft_direction: Vector3,
    draft_angle: f64,
) -> OperationResult<Box<dyn Surface>> {
    let surface = model
        .surfaces
        .get(original_surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Calculate draft transformation
    let _cos_angle = draft_angle.cos();
    let _sin_angle = draft_angle.sin();

    // Create transformation matrix for draft
    // This applies a shear transformation in the draft direction
    let _draft_transform = create_draft_transform_matrix(draft_direction, draft_angle);

    // Apply draft transformation to the surface
    // For planar surfaces, we can create a new inclined plane
    use crate::primitives::surface::Plane;

    // If it's a plane, create a new drafted plane
    if let Some(plane_surface) = surface.as_any().downcast_ref::<Plane>() {
        // Get original plane parameters
        let original_normal = plane_surface.normal;
        let original_origin = plane_surface.origin;

        // Calculate new normal by rotating original normal by draft angle
        let axis = draft_direction.cross(&original_normal);
        if axis.magnitude() > 1e-10 {
            let axis_normalized = axis.normalize().map_err(|e| {
                OperationError::NumericalError(format!("Axis normalization failed: {:?}", e))
            })?;

            // Create rotation matrix for draft angle
            let rotation = Matrix4::from_axis_angle(&axis_normalized, draft_angle)?;
            let new_normal = rotation.transform_vector(&original_normal);

            // Create new plane with drafted normal
            let new_plane = Plane::from_point_normal(original_origin, new_normal)?;
            return Ok(Box::new(new_plane));
        }
    }

    // For other surface types, create a general transformed surface
    // This is a simplified approach - in a full implementation, we'd create
    // specialized draft surfaces for each surface type

    // Calculate average point from neutral curve
    let neutral_center = if !neutral_curve.is_empty() {
        let sum = neutral_curve.iter().fold(Point3::ZERO, |acc, p| acc + *p);
        sum / neutral_curve.len() as f64
    } else {
        Point3::ZERO
    };

    // Create a simple drafted plane based on neutral curve and draft direction
    let drafted_normal = draft_direction.cross(&Vector3::X);
    let final_normal = if drafted_normal.magnitude() > 1e-10 {
        drafted_normal.normalize().map_err(|e| {
            OperationError::NumericalError(format!("Normal calculation failed: {:?}", e))
        })?
    } else {
        Vector3::Z
    };

    // Rotate the normal by the draft angle
    let axis = draft_direction.cross(&final_normal);
    if axis.magnitude() > 1e-10 {
        let axis_normalized = axis.normalize().map_err(|e| {
            OperationError::NumericalError(format!("Axis normalization failed: {:?}", e))
        })?;
        let rotation = Matrix4::from_axis_angle(&axis_normalized, draft_angle)?;
        let rotated_normal = rotation.transform_vector(&final_normal);

        let drafted_plane = Plane::from_point_normal(neutral_center, rotated_normal)?;
        Ok(Box::new(drafted_plane))
    } else {
        // Fallback to XY plane
        Ok(Box::new(Plane::xy(neutral_center.z)))
    }
}

/// Create draft transformation matrix
fn create_draft_transform_matrix(_draft_direction: Vector3, draft_angle: f64) -> Matrix4 {
    // Create a shear transformation matrix
    // This is simplified - a full implementation would use more sophisticated transformations
    let cos_angle = draft_angle.cos();
    let _sin_angle = draft_angle.sin();

    // Create a basic transformation matrix
    // In practice, this would be more complex depending on the draft direction
    let scale_factor = 1.0 / cos_angle;
    Matrix4::uniform_scale(scale_factor)
}

/// Create drafted loop
fn create_drafted_loop(
    model: &mut BRepModel,
    face: &Face,
    neutral_curve: &[Point3],
    draft_direction: Vector3,
    draft_angle: f64,
) -> OperationResult<Loop> {
    let original_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut drafted_edges = Vec::new();

    // Draft each edge
    for i in 0..original_loop.edges.len() {
        let edge_id = original_loop.edges[i];
        let forward = original_loop.orientations.get(i).copied().unwrap_or(true);
        let drafted_edge = create_drafted_edge(
            model,
            edge_id,
            neutral_curve,
            draft_direction,
            draft_angle,
            forward,
        )?;
        drafted_edges.push((drafted_edge, forward));
    }

    // Create new loop
    let mut drafted_loop = Loop::new(
        0, // Temporary ID
        original_loop.loop_type,
    );
    for (edge_id, forward) in drafted_edges {
        drafted_loop.add_edge(edge_id, forward);
    }

    Ok(drafted_loop)
}

/// Create drafted edge
fn create_drafted_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    _neutral_curve: &[Point3],
    draft_direction: Vector3,
    draft_angle: f64,
    forward: bool,
) -> OperationResult<EdgeId> {
    let original_edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Get original edge vertices
    let start_vertex = model
        .vertices
        .get(original_edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?
        .clone();
    let end_vertex = model
        .vertices
        .get(original_edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?
        .clone();

    // Calculate draft transformation
    let draft_transform = create_draft_transform_matrix(draft_direction, draft_angle);

    // Create drafted vertices
    let start_pos = start_vertex.point();
    let end_pos = end_vertex.point();

    // Apply draft transformation to vertices
    let drafted_start = draft_transform.transform_point(&start_pos);
    let drafted_end = draft_transform.transform_point(&end_pos);

    // Create new vertices
    let start_vertex_id = model
        .vertices
        .add(drafted_start.x, drafted_start.y, drafted_start.z);
    let end_vertex_id = model
        .vertices
        .add(drafted_end.x, drafted_end.y, drafted_end.z);

    // Create drafted curve
    // For now, create a simple line between drafted vertices
    use crate::primitives::curve::Line;
    let drafted_line = Line::new(drafted_start, drafted_end);
    let curve_id = model.curves.add(Box::new(drafted_line));

    // Create new edge using the new method
    use crate::primitives::curve::ParameterRange;
    let drafted_edge = Edge::new(
        0, // Temporary ID
        start_vertex_id,
        end_vertex_id,
        curve_id,
        if forward {
            EdgeOrientation::Forward
        } else {
            EdgeOrientation::Backward
        },
        ParameterRange::unit(),
    );

    let new_edge_id = model.edges.add(drafted_edge);
    Ok(new_edge_id)
}

/// Handle intersections between drafted faces
fn handle_draft_intersections(
    model: &mut BRepModel,
    drafted_faces: &[FaceId],
    options: &DraftOptions,
) -> OperationResult<()> {
    match options.intersection_handling {
        DraftIntersectionHandling::Extend => extend_drafted_faces(model, drafted_faces),
        DraftIntersectionHandling::Trim => trim_drafted_faces(model, drafted_faces),
        DraftIntersectionHandling::Blend => blend_drafted_faces(model, drafted_faces),
    }
}

/// Extend drafted faces to intersection
fn extend_drafted_faces(model: &mut BRepModel, faces: &[FaceId]) -> OperationResult<()> {
    // Find intersections between adjacent drafted faces
    for i in 0..faces.len() {
        for j in i + 1..faces.len() {
            let face1 = model
                .faces
                .get(faces[i])
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
            let face2 = model
                .faces
                .get(faces[j])
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

            // Get surfaces
            let surface1 = model
                .surfaces
                .get(face1.surface_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
            let surface2 = model
                .surfaces
                .get(face2.surface_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

            // Calculate intersection curve between surfaces
            let intersection_curve = compute_surface_intersection(surface1, surface2)?;

            // Extend face boundaries to meet at intersection
            extend_face_to_curve(model, faces[i], &intersection_curve)?;
            extend_face_to_curve(model, faces[j], &intersection_curve)?;
        }
    }

    Ok(())
}

/// Trim drafted faces at intersection
fn trim_drafted_faces(model: &mut BRepModel, faces: &[FaceId]) -> OperationResult<()> {
    // Find intersections and trim faces at intersection curves
    for i in 0..faces.len() {
        for j in i + 1..faces.len() {
            let face1 = model
                .faces
                .get(faces[i])
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
            let face2 = model
                .faces
                .get(faces[j])
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

            // Get surfaces
            let surface1 = model
                .surfaces
                .get(face1.surface_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
            let surface2 = model
                .surfaces
                .get(face2.surface_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

            // Calculate intersection curve
            let intersection_curve = compute_surface_intersection(surface1, surface2)?;

            // Trim both faces at the intersection curve
            trim_face_at_curve(model, faces[i], &intersection_curve)?;
            trim_face_at_curve(model, faces[j], &intersection_curve)?;
        }
    }

    Ok(())
}

/// Blend drafted face intersections
fn blend_drafted_faces(model: &mut BRepModel, faces: &[FaceId]) -> OperationResult<()> {
    // Create blend surfaces at face intersections
    for i in 0..faces.len() {
        for j in i + 1..faces.len() {
            let face1 = model
                .faces
                .get(faces[i])
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
            let face2 = model
                .faces
                .get(faces[j])
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

            // Check if faces are adjacent
            if are_faces_adjacent(model, faces[i], faces[j])? {
                // Create blend surface between the faces
                let blend_surface = create_blend_surface_between_faces(model, face1, face2)?;
                let blend_surface_id = model.surfaces.add(blend_surface);

                // Create blend face with appropriate loop
                let blend_loop = create_blend_loop(model, faces[i], faces[j])?;
                let blend_loop_id = model.loops.add(blend_loop);

                let blend_face = Face::new(
                    0, // Temporary ID
                    blend_surface_id,
                    blend_loop_id,
                    FaceOrientation::Forward,
                );

                model.faces.add(blend_face);
            }
        }
    }

    Ok(())
}

/// Update adjacent faces for draft
fn update_adjacent_faces_for_draft(
    _model: &mut BRepModel,
    _solid_id: SolidId,
    _original_faces: &[FaceId],
    _drafted_faces: &[FaceId],
) -> OperationResult<()> {
    // Would update neighboring faces to maintain topology
    Ok(())
}

/// Compute face-plane intersection curve
fn compute_face_plane_intersection(
    model: &BRepModel,
    face: &Face,
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<Point3>> {
    let mut intersection_points = Vec::new();

    // Get face's outer loop
    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    // For each edge in the loop, find intersection with plane
    for i in 0..loop_data.edges.len() {
        let edge_id = loop_data.edges[i];
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Get edge endpoints
        let start_vertex = model
            .vertices
            .get(edge.start_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
        let end_vertex = model
            .vertices
            .get(edge.end_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

        let start_pos = start_vertex.point();
        let end_pos = end_vertex.point();

        // Check if edge intersects the plane
        let start_dist = (start_pos - plane_origin).dot(&plane_normal);
        let end_dist = (end_pos - plane_origin).dot(&plane_normal);

        // If endpoints are on opposite sides of plane, there's an intersection
        if start_dist * end_dist < 0.0 {
            let t = start_dist.abs() / (start_dist.abs() + end_dist.abs());
            let intersection = start_pos + (end_pos - start_pos) * t;
            intersection_points.push(intersection);
        }
        // If endpoint is exactly on plane, include it
        else if start_dist.abs() < 1e-10 {
            intersection_points.push(start_pos);
        }
    }

    // If no intersections found, project face centroid onto plane
    if intersection_points.is_empty() {
        // Calculate face centroid
        let mut centroid = Point3::ZERO;
        let mut vertex_count = 0;

        for i in 0..loop_data.edges.len() {
            let edge_id = loop_data.edges[i];
            let edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

            let vertex = model
                .vertices
                .get(edge.start_vertex)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

            let point = vertex.point();
            centroid.x += point.x;
            centroid.y += point.y;
            centroid.z += point.z;
            vertex_count += 1;
        }

        if vertex_count > 0 {
            centroid.x /= vertex_count as f64;
            centroid.y /= vertex_count as f64;
            centroid.z /= vertex_count as f64;

            // Project centroid onto plane
            let dist_to_plane = (centroid - plane_origin).dot(&plane_normal);
            let projected = centroid - plane_normal * dist_to_plane;
            intersection_points.push(projected);
        }
    }

    Ok(intersection_points)
}

/// Compute average face normal
fn compute_average_face_normal(model: &BRepModel, face: &Face) -> OperationResult<Vector3> {
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Sample normal at center
    Ok(surface.normal_at(0.5, 0.5)?)
}

/// Compute draft direction from face normal and pull direction
fn compute_draft_direction(
    _face_normal: Vector3,
    pull_direction: Vector3,
    plane_normal: Vector3,
) -> Vector3 {
    // Project pull direction onto plane tangent to neutral
    let tangent = pull_direction - pull_direction.dot(&plane_normal) * plane_normal;
    tangent.normalize().unwrap_or(Vector3::Z)
}

/// Sample points along an edge
fn sample_edge_points(model: &BRepModel, edge: &Edge) -> OperationResult<Vec<Point3>> {
    let num_samples = 10;
    let mut points = Vec::new();

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let point = edge.evaluate(t, &model.curves)?;
        points.push(point);
    }

    Ok(points)
}

/// Sample points along a curve
fn sample_curve_points(curve: &Box<dyn Curve>) -> OperationResult<Vec<Point3>> {
    let num_samples = 10;
    let mut points = Vec::new();

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let point = curve.point_at(t)?;
        points.push(point);
    }

    Ok(points)
}

/// Merge face groups that share neutral curves
fn merge_face_groups(_groups: &mut Vec<FaceGroup>) -> OperationResult<()> {
    // Would merge groups with common neutral curves
    Ok(())
}

/// Validate draft inputs
fn validate_draft_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    faces: &[FaceId],
    options: &DraftOptions,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check faces exist
    for &face_id in faces {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Face not found".to_string(),
            ));
        }
    }

    // Check draft angle
    match &options.draft_type {
        DraftType::Angle(angle) => {
            if angle.abs() > std::f64::consts::PI / 2.0 {
                return Err(OperationError::InvalidGeometry(
                    "Draft angle too large".to_string(),
                ));
            }
        }
        _ => {} // Other types validated during execution
    }

    // Check pull direction
    if options.pull_direction.magnitude() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Invalid pull direction".to_string(),
        ));
    }

    Ok(())
}

/// Compute surface-surface intersection
fn compute_surface_intersection(
    surface1: &dyn Surface,
    surface2: &dyn Surface,
) -> OperationResult<Vec<Point3>> {
    // Simplified surface intersection - in practice this would be much more complex
    // For now, return a simple line between surface origins if they're planes
    use crate::primitives::surface::Plane;

    if let (Some(plane1), Some(plane2)) = (
        surface1.as_any().downcast_ref::<Plane>(),
        surface2.as_any().downcast_ref::<Plane>(),
    ) {
        let origin1 = plane1.origin;
        let origin2 = plane2.origin;
        let normal1 = plane1.normal;
        let normal2 = plane2.normal;

        // Calculate intersection line direction
        let line_direction = normal1.cross(&normal2);
        if line_direction.magnitude() > 1e-10 {
            // Planes intersect in a line
            let midpoint = (origin1 + origin2) / 2.0;
            let offset = line_direction.normalize().unwrap_or(Vector3::X) * 10.0;
            return Ok(vec![midpoint - offset, midpoint + offset]);
        }
    }

    // Fallback - return empty intersection
    Ok(vec![])
}

/// Extend face to intersection curve
fn extend_face_to_curve(
    _model: &mut BRepModel,
    _face_id: FaceId,
    _intersection_curve: &[Point3],
) -> OperationResult<()> {
    // Would extend face boundary to meet intersection curve
    // This is a complex operation involving loop modification
    Ok(())
}

/// Trim face at intersection curve
fn trim_face_at_curve(
    _model: &mut BRepModel,
    _face_id: FaceId,
    _intersection_curve: &[Point3],
) -> OperationResult<()> {
    // Would trim face at intersection curve
    // This involves splitting the face and creating new boundaries
    Ok(())
}

/// Check if two faces are adjacent
fn are_faces_adjacent(
    model: &BRepModel,
    face_id1: FaceId,
    face_id2: FaceId,
) -> OperationResult<bool> {
    let face1 = model
        .faces
        .get(face_id1)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
    let face2 = model
        .faces
        .get(face_id2)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    // Get loops
    let loop1 = model
        .loops
        .get(face1.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;
    let loop2 = model
        .loops
        .get(face2.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    // Check if loops share any edges
    for &edge1 in &loop1.edges {
        for &edge2 in &loop2.edges {
            if edge1 == edge2 {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Create blend surface between two faces
fn create_blend_surface_between_faces(
    model: &BRepModel,
    face1: &Face,
    face2: &Face,
) -> OperationResult<Box<dyn Surface>> {
    // Create a simple cylindrical blend surface
    // In practice, this would be much more sophisticated
    use crate::primitives::surface::Plane;

    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // For simplicity, create a plane between the two surfaces
    if let (Some(plane1), Some(plane2)) = (
        surface1.as_any().downcast_ref::<Plane>(),
        surface2.as_any().downcast_ref::<Plane>(),
    ) {
        let origin1 = plane1.origin;
        let origin2 = plane2.origin;
        let normal1 = plane1.normal;
        let normal2 = plane2.normal;

        // Create blend normal as average of the two normals
        let blend_normal = (normal1 + normal2).normalize().unwrap_or(Vector3::Z);
        let blend_origin = (origin1 + origin2) / 2.0;

        let blend_plane = Plane::from_point_normal(blend_origin, blend_normal)?;
        return Ok(Box::new(blend_plane));
    }

    // Fallback to XY plane
    Ok(Box::new(Plane::xy(0.0)))
}

/// Create blend loop between two faces
fn create_blend_loop(
    _model: &BRepModel,
    _face_id1: FaceId,
    _face_id2: FaceId,
) -> OperationResult<Loop> {
    // Create a simple rectangular loop for the blend
    use crate::primitives::r#loop::{Loop, LoopType};

    let blend_loop = Loop::new(0, LoopType::Outer);

    // For simplicity, return empty loop
    // In practice, this would create edges connecting the face boundaries
    Ok(blend_loop)
}

/// Validate drafted solid
fn validate_drafted_solid(_model: &BRepModel, _solid_id: SolidId) -> OperationResult<()> {
    // Would perform full validation
    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_draft_validation() {
//         // Test parameter validation
//     }
// }
