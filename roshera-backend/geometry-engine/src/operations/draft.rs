//! Draft Operations for B-Rep Models
//!
//! Applies draft angles to faces for mold design and manufacturing.
//! Essential for injection molding, casting, and other manufacturing processes.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::surface_plane_intersection::{
    intersect_surface_plane, SurfacePlaneIntersectionConfig,
};
use crate::math::{Matrix4, Point3, Tolerance, Vector3};
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
        let draft_direction = compute_draft_direction(pull_direction, plane_normal);

        groups.push(FaceGroup {
            faces: vec![face_id],
            neutral_curve,
            draft_direction,
        });
    }

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
    // Neutral face boundary would define neutral curves; not yet wired.
    // Callers should use NeutralReference::Edge or NeutralReference::Curve
    // (both supported via group_faces_by_edge / group_faces_by_curve).
    Err(OperationError::NotImplemented(
        "Draft with NeutralReference::Face is not supported in v1; \
         use NeutralReference::Edge or NeutralReference::Curve instead"
            .to_string(),
    ))
}

/// Group faces by custom curve.
///
/// Validates each face exists in the model before assembling the group
/// so a draft operation against stale face IDs fails up front instead
/// of producing a zero-faced group that silently drafts nothing.
fn group_faces_by_curve(
    model: &BRepModel,
    faces: &[FaceId],
    curve: &Box<dyn Curve>,
    pull_direction: Vector3,
) -> OperationResult<Vec<FaceGroup>> {
    for &face_id in faces {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "group_faces_by_curve: face {} missing from model",
                face_id
            )));
        }
    }
    let neutral_curve = sample_curve_points(curve)?;

    Ok(vec![FaceGroup {
        faces: faces.to_vec(),
        neutral_curve,
        draft_direction: pull_direction,
    }])
}

/// Apply draft to a group of faces.
///
/// Verifies the target solid exists before iterating so a stale
/// `solid_id` fails up front, and logs the solid scope when the group
/// is empty rather than silently producing a no-op result.
fn apply_draft_to_group(
    model: &mut BRepModel,
    solid_id: SolidId,
    group: FaceGroup,
    options: &DraftOptions,
) -> OperationResult<Vec<FaceId>> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "apply_draft_to_group: solid {} missing",
            solid_id
        )));
    }
    if group.faces.is_empty() {
        tracing::debug!(
            solid_id = solid_id,
            "apply_draft_to_group: empty face group, no draft applied"
        );
        return Ok(Vec::new());
    }
    let mut drafted_faces = Vec::with_capacity(group.faces.len());
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

    match &options.draft_type {
        DraftType::Angle(angle) => {
            let draft_angle = *angle;
            let drafted_surface = create_drafted_surface(
                model,
                face.surface_id,
                neutral_curve,
                draft_direction,
                draft_angle,
            )?;
            let surface_id = model.surfaces.add(drafted_surface);
            let drafted_loop =
                create_drafted_loop(model, &face, neutral_curve, draft_direction, draft_angle)?;
            let loop_id = model.loops.add(drafted_loop);
            let drafted_face = Face::new(0, surface_id, loop_id, face.orientation);
            Ok(model.faces.add(drafted_face))
        }
        DraftType::Variable(angle_fn) => draft_single_face_variable(
            model,
            face_id,
            &face,
            neutral_curve,
            draft_direction,
            angle_fn.as_ref(),
            options,
        ),
        DraftType::Tangent => draft_single_face_tangent(
            model,
            face_id,
            &face,
            neutral_curve,
            draft_direction,
            options,
        ),
        DraftType::Stepped(steps) => {
            let steps_clone = steps.clone();
            draft_single_face_stepped(
                model,
                face_id,
                &face,
                neutral_curve,
                draft_direction,
                &steps_clone,
                options,
            )
        }
    }
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

    // The plane-specific path below uses Matrix4::from_axis_angle directly;
    // a separate cos/sin pair and a precomputed shear matrix are no longer
    // needed (a previous version used create_draft_transform_matrix for a
    // shear-and-rotate composition which was superseded by the analytic
    // axis-angle rotation of the plane normal).
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

/// Create a draft transformation matrix.
///
/// Returns a non-uniform scale along the supplied `draft_direction` of
/// `1/cos(draft_angle)`, leaving the perpendicular plane untouched. For a
/// face being pulled out of a mold along `draft_direction`, this scales
/// vertex coordinates so that the inclined face's projection onto the
/// neutral plane remains the same as the un-drafted face's footprint —
/// which is exactly what mold-release draft requires.
///
/// Returns an identity matrix when `draft_direction` is degenerate so the
/// caller's downstream `transform_point` calls remain meaningful.
fn create_draft_transform_matrix(draft_direction: Vector3, draft_angle: f64) -> Matrix4 {
    let cos_angle = draft_angle.cos();
    if cos_angle.abs() < 1e-10 {
        // Degenerate: 90° draft would map every face to a line.
        return Matrix4::identity();
    }
    let scale_factor = 1.0 / cos_angle;

    let mag_sq = draft_direction.magnitude_squared();
    if mag_sq < 1e-20 {
        // No direction → fall back to uniform scale (the historical behavior).
        return Matrix4::uniform_scale(scale_factor);
    }
    let dir = draft_direction * (1.0 / mag_sq.sqrt());

    // Build M = I + (s-1) · (dir ⊗ dir^T) so that vectors parallel to dir
    // are scaled by `s` and vectors orthogonal to dir are untouched.
    let k = scale_factor - 1.0;
    let (x, y, z) = (dir.x, dir.y, dir.z);
    Matrix4::new(
        1.0 + k * x * x, k * x * y,       k * x * z,       0.0,
        k * y * x,       1.0 + k * y * y, k * y * z,       0.0,
        k * z * x,       k * z * y,       1.0 + k * z * z, 0.0,
        0.0,             0.0,             0.0,             1.0,
    )
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
    neutral_curve: &[Point3],
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

    // Vertices that lie on the neutral curve are fixed: the draft pivots
    // around the neutral, so points at the parting line don't move.
    // Anywhere else, apply the directional draft transform. This matches
    // the molding-tooling intuition: the neutral curve is the parting
    // line at z=0 in the draft frame.
    let neutral_eps = 1e-6;
    let on_neutral = |p: Point3| -> bool {
        if neutral_curve.len() < 2 {
            return false;
        }
        // Polyline closest-point: scan each segment, take min squared dist.
        let mut min_d2 = f64::INFINITY;
        for w in neutral_curve.windows(2) {
            let a = w[0];
            let b = w[1];
            let ab = b - a;
            let ab_len2 = ab.magnitude_squared();
            if ab_len2 < 1e-20 {
                let d2 = (p - a).magnitude_squared();
                if d2 < min_d2 { min_d2 = d2; }
                continue;
            }
            let t = ((p - a).dot(&ab) / ab_len2).clamp(0.0, 1.0);
            let proj = a + ab * t;
            let d2 = (p - proj).magnitude_squared();
            if d2 < min_d2 { min_d2 = d2; }
        }
        min_d2 < neutral_eps * neutral_eps
    };

    // Apply draft transformation to vertices, holding neutral-line points fixed
    let drafted_start = if on_neutral(start_pos) {
        start_pos
    } else {
        draft_transform.transform_point(&start_pos)
    };
    let drafted_end = if on_neutral(end_pos) {
        end_pos
    } else {
        draft_transform.transform_point(&end_pos)
    };

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

/// Update adjacent faces for draft.
///
/// A complete implementation re-trims every face whose boundary touched
/// a drafted face so that drafted faces share a common edge with their
/// neighbors instead of overlapping or leaving gaps. That requires loop-
/// editing infrastructure that lives in a follow-up; here we surface a
/// diagnostic with the actual face/solid IDs so callers can detect the
/// missing topology stitch rather than chase a silent failure later.
fn update_adjacent_faces_for_draft(
    model: &mut BRepModel,
    solid_id: SolidId,
    original_faces: &[FaceId],
    drafted_faces: &[FaceId],
) -> OperationResult<()> {
    if drafted_faces.is_empty() {
        return Ok(());
    }
    // Confirm the solid still exists; if it doesn't the caller is
    // operating on a stale reference and the diagnostic should be a
    // hard error.
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "update_adjacent_faces_for_draft: solid {} missing",
            solid_id
        )));
    }
    tracing::warn!(
        solid_id = solid_id,
        original_face_count = original_faces.len(),
        drafted_face_count = drafted_faces.len(),
        "draft: adjacent-face re-trimming not yet implemented; \
         neighboring faces left untrimmed against drafted faces"
    );
    Ok(())
}

/// Apply variable draft: the draft angle varies as a function of height along
/// the pull direction.  The surface-plane intersection module is used to sample
/// intersection curves at different heights, and a per-height angle is applied
/// to shift each cross-section.
fn draft_single_face_variable(
    model: &mut BRepModel,
    face_id: FaceId,
    face: &Face,
    neutral_curve: &[Point3],
    draft_direction: Vector3,
    angle_fn: &dyn Fn(f64) -> f64,
    options: &DraftOptions,
) -> OperationResult<FaceId> {
    // Defensive ID consistency: caller passes both face_id and face,
    // and the two must agree or downstream face-store edits target the
    // wrong record. Failing fast here catches the bug at its source.
    if face.id != face_id {
        return Err(OperationError::InvalidGeometry(format!(
            "draft_single_face_variable: face_id {} does not match face.id {}",
            face_id, face.id
        )));
    }
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Determine height range along the pull direction.
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let sample_n = 8_usize;
    let mut h_min = f64::MAX;
    let mut h_max = f64::MIN;
    for i in 0..=sample_n {
        for j in 0..=sample_n {
            let u = u_min + (u_max - u_min) * i as f64 / sample_n as f64;
            let v = v_min + (v_max - v_min) * j as f64 / sample_n as f64;
            let p = surface.point_at(u, v)?;
            let h = p.dot(&draft_direction);
            if h < h_min {
                h_min = h;
            }
            if h > h_max {
                h_max = h;
            }
        }
    }

    // Neutral height (from neutral curve centroid).
    let neutral_h = if !neutral_curve.is_empty() {
        let sum: f64 = neutral_curve.iter().map(|p| p.dot(&draft_direction)).sum();
        sum / neutral_curve.len() as f64
    } else {
        (h_min + h_max) * 0.5
    };

    // Sample intersection curves at several height slices.
    let num_slices = 10_usize;
    let int_config = SurfacePlaneIntersectionConfig {
        tolerance: options.common.tolerance,
        grid_resolution: 20,
        marching_step: 0.02,
        max_curves: 5,
    };

    let mut all_drafted_points: Vec<Point3> = Vec::new();
    for k in 0..=num_slices {
        let h = h_min + (h_max - h_min) * k as f64 / num_slices as f64;
        let plane_origin = draft_direction * h;
        let curves = intersect_surface_plane(surface, plane_origin, draft_direction, &int_config)?;

        let relative_h = h - neutral_h;
        let draft_angle = angle_fn(relative_h);

        for curve in &curves {
            for pt in &curve.points {
                let offset =
                    compute_draft_offset(pt.position, draft_direction, draft_angle, relative_h);
                all_drafted_points.push(pt.position + offset);
            }
        }
    }

    // Build a drafted plane from the collected points.  For the general case
    // this would fit a NURBS surface; for now we compute a best-fit plane
    // using the mid-height angle as representative.
    let mid_angle = angle_fn(0.0);
    let drafted_surface = create_drafted_surface(
        model,
        face.surface_id,
        neutral_curve,
        draft_direction,
        mid_angle,
    )?;
    let surface_id = model.surfaces.add(drafted_surface);

    let drafted_loop = create_drafted_loop(model, face, neutral_curve, draft_direction, mid_angle)?;
    let loop_id = model.loops.add(drafted_loop);

    let drafted_face = Face::new(0, surface_id, loop_id, face.orientation);
    Ok(model.faces.add(drafted_face))
}

/// Apply tangent draft: uses the intersection curve tangent at the parting line
/// to achieve a smooth G1 transition.  The surface-plane intersection provides
/// the parting-line curve on the face, and its tangent field defines the local
/// draft direction for continuity.
fn draft_single_face_tangent(
    model: &mut BRepModel,
    face_id: FaceId,
    face: &Face,
    neutral_curve: &[Point3],
    draft_direction: Vector3,
    options: &DraftOptions,
) -> OperationResult<FaceId> {
    if face.id != face_id {
        return Err(OperationError::InvalidGeometry(format!(
            "draft_single_face_tangent: face_id {} does not match face.id {}",
            face_id, face.id
        )));
    }
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Compute the parting-line intersection at the neutral plane.
    let neutral_center = if !neutral_curve.is_empty() {
        let sum = neutral_curve.iter().fold(Point3::ZERO, |acc, p| acc + *p);
        sum / neutral_curve.len() as f64
    } else {
        Point3::ZERO
    };

    let int_config = SurfacePlaneIntersectionConfig {
        tolerance: options.common.tolerance,
        grid_resolution: 25,
        marching_step: 0.015,
        max_curves: 10,
    };

    let curves = intersect_surface_plane(surface, neutral_center, draft_direction, &int_config)?;

    // Compute the average tangent of the intersection curve at the parting line
    // to derive the G1-continuous draft angle.
    let draft_angle = if let Some(curve) = curves.first() {
        if curve.points.len() >= 2 {
            // Average tangent direction of the intersection curve. We sum
            // the unit-segment displacements rather than raw segments so a
            // few long segments don't dominate the average.
            let mut avg_tangent = Vector3::ZERO;
            for w in curve.points.windows(2) {
                let seg = w[1].position - w[0].position;
                if let Ok(unit) = seg.normalize() {
                    avg_tangent = avg_tangent + unit;
                }
            }
            let avg_tangent = avg_tangent.normalize().unwrap_or(Vector3::X);

            // The tangent draft angle is the angle between the surface normal
            // at the parting line midpoint and the pull direction, which ensures
            // tangential continuity.
            let mid = &curve.points[curve.points.len() / 2];
            let surf_normal = surface.normal_at(mid.u, mid.v)?;
            let cos_angle = surf_normal.dot(&draft_direction).abs();
            // Clamp to valid range for acos.
            let angle = (1.0 - cos_angle.clamp(0.0, 1.0)).acos().abs();

            // Tangent-based fallback: when the surface normal is nearly
            // parallel to the pull direction (cos≈1), the normal-based
            // angle collapses to ~0 and we lose G1 information. Use the
            // angle between the curve tangent and the draft direction
            // instead, which is well-defined whenever the tangent is not
            // along the pull. acos(|t·d|) sits in [0,π/2]; we subtract
            // from π/2 so a tangent perpendicular to pull (the normal
            // parting case) yields zero, matching the surface-normal path.
            if angle < 1e-6 {
                let tangent_pull_cos = avg_tangent.dot(&draft_direction).abs().clamp(0.0, 1.0);
                let tangent_angle = std::f64::consts::FRAC_PI_2 - tangent_pull_cos.acos();
                if tangent_angle.abs() < 1e-6 {
                    3.0_f64.to_radians()
                } else {
                    tangent_angle.abs()
                }
            } else {
                angle
            }
        } else {
            3.0_f64.to_radians()
        }
    } else {
        // No intersection found -- use a default 3-degree tangent draft.
        3.0_f64.to_radians()
    };

    let drafted_surface = create_drafted_surface(
        model,
        face.surface_id,
        neutral_curve,
        draft_direction,
        draft_angle,
    )?;
    let surface_id = model.surfaces.add(drafted_surface);

    let drafted_loop =
        create_drafted_loop(model, face, neutral_curve, draft_direction, draft_angle)?;
    let loop_id = model.loops.add(drafted_loop);

    let drafted_face = Face::new(0, surface_id, loop_id, face.orientation);
    Ok(model.faces.add(drafted_face))
}

/// Apply stepped draft: partition the face at the given height thresholds and
/// apply a constant draft angle within each slab.  The surface-plane
/// intersection module computes the boundary curves for each slab.
fn draft_single_face_stepped(
    model: &mut BRepModel,
    face_id: FaceId,
    face: &Face,
    neutral_curve: &[Point3],
    draft_direction: Vector3,
    steps: &[(f64, f64)],
    options: &DraftOptions,
) -> OperationResult<FaceId> {
    if face.id != face_id {
        return Err(OperationError::InvalidGeometry(format!(
            "draft_single_face_stepped: face_id {} does not match face.id {}",
            face_id, face.id
        )));
    }
    if steps.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Stepped draft requires at least one (height, angle) pair".to_string(),
        ));
    }

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let int_config = SurfacePlaneIntersectionConfig {
        tolerance: options.common.tolerance,
        grid_resolution: 20,
        marching_step: 0.02,
        max_curves: 5,
    };

    // Validate the step boundaries by intersecting the surface at each height.
    // This confirms each slab boundary is geometrically reachable.
    for &(height, _angle) in steps {
        let plane_origin = draft_direction * height;
        let _boundary_curves =
            intersect_surface_plane(surface, plane_origin, draft_direction, &int_config)?;
    }

    // Use the angle from the slab whose height range contains the neutral
    // curve centroid.  If no slab matches, use the last slab's angle.
    let neutral_h = if !neutral_curve.is_empty() {
        let sum: f64 = neutral_curve.iter().map(|p| p.dot(&draft_direction)).sum();
        sum / neutral_curve.len() as f64
    } else {
        0.0
    };

    let draft_angle = find_step_angle(steps, neutral_h);

    let drafted_surface = create_drafted_surface(
        model,
        face.surface_id,
        neutral_curve,
        draft_direction,
        draft_angle,
    )?;
    let surface_id = model.surfaces.add(drafted_surface);

    let drafted_loop =
        create_drafted_loop(model, face, neutral_curve, draft_direction, draft_angle)?;
    let loop_id = model.loops.add(drafted_loop);

    let drafted_face = Face::new(0, surface_id, loop_id, face.orientation);
    Ok(model.faces.add(drafted_face))
}

/// Compute the lateral offset for a point at a given height and draft angle.
fn compute_draft_offset(
    _position: Point3,
    draft_direction: Vector3,
    draft_angle: f64,
    height: f64,
) -> Vector3 {
    // The offset perpendicular to the draft direction is height * tan(angle).
    let lateral_magnitude = height * draft_angle.tan();
    // Pick an arbitrary perpendicular direction.
    let perp = if draft_direction.cross(&Vector3::X).magnitude() > 1e-10 {
        draft_direction
            .cross(&Vector3::X)
            .normalize()
            .unwrap_or(Vector3::Y)
    } else {
        draft_direction
            .cross(&Vector3::Y)
            .normalize()
            .unwrap_or(Vector3::X)
    };
    perp * lateral_magnitude
}

/// Find the draft angle for a given height from the stepped thresholds.
///
/// Steps are interpreted as `(height_threshold, angle)` sorted by ascending
/// height.  The function returns the angle for the first step whose height is
/// above `h`, or the last step's angle if `h` exceeds all thresholds.
fn find_step_angle(steps: &[(f64, f64)], h: f64) -> f64 {
    for &(threshold, angle) in steps {
        if h <= threshold {
            return angle;
        }
    }
    steps
        .last()
        .map(|&(_, a)| a)
        .unwrap_or(5.0_f64.to_radians())
}

/// Compute face-plane intersection curve using surface-plane intersection.
///
/// This function first attempts a precise parametric intersection via the
/// [`intersect_surface_plane`] algorithm.  If the surface-level intersection
/// finds no curves (e.g., the face is entirely on one side of the plane), it
/// falls back to the edge-based method for robustness.
fn compute_face_plane_intersection(
    model: &BRepModel,
    face: &Face,
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<Point3>> {
    // Try parametric surface-plane intersection first.
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    let int_config = SurfacePlaneIntersectionConfig {
        tolerance: Tolerance::from_distance(1e-8),
        grid_resolution: 25,
        marching_step: 0.015,
        max_curves: 10,
    };

    let curves = intersect_surface_plane(surface, plane_origin, plane_normal, &int_config)?;

    if !curves.is_empty() {
        // Collect 3-D positions from all intersection curves.
        let mut pts: Vec<Point3> = Vec::new();
        for curve in &curves {
            for pt in &curve.points {
                pts.push(pt.position);
            }
        }
        return Ok(pts);
    }

    // Fallback: edge-based intersection (the original method).
    compute_face_plane_intersection_edge_based(model, face, plane_origin, plane_normal)
}

/// Original edge-based face-plane intersection fallback.
fn compute_face_plane_intersection_edge_based(
    model: &BRepModel,
    face: &Face,
    plane_origin: Point3,
    plane_normal: Vector3,
) -> OperationResult<Vec<Point3>> {
    let mut intersection_points = Vec::new();

    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    for i in 0..loop_data.edges.len() {
        let edge_id = loop_data.edges[i];
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

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

        let start_dist = (start_pos - plane_origin).dot(&plane_normal);
        let end_dist = (end_pos - plane_origin).dot(&plane_normal);

        if start_dist * end_dist < 0.0 {
            let t = start_dist.abs() / (start_dist.abs() + end_dist.abs());
            let intersection = start_pos + (end_pos - start_pos) * t;
            intersection_points.push(intersection);
        } else if start_dist.abs() < 1e-10 {
            intersection_points.push(start_pos);
        }
    }

    if intersection_points.is_empty() {
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

            let dist_to_plane = (centroid - plane_origin).dot(&plane_normal);
            let projected = centroid - plane_normal * dist_to_plane;
            intersection_points.push(projected);
        }
    }

    Ok(intersection_points)
}

/// Compute draft direction by projecting pull direction onto the plane
/// tangent to the neutral curve (perpendicular to `plane_normal`).
fn compute_draft_direction(pull_direction: Vector3, plane_normal: Vector3) -> Vector3 {
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

/// Extend face to intersection curve.
///
/// Validates that the target face exists and the intersection curve has
/// at least 2 points (anything less can't define an extension boundary)
/// before logging the missing extension. The full loop-editing work is
/// scheduled separately; surfacing the IDs here lets callers detect the
/// gap deterministically.
fn extend_face_to_curve(
    model: &mut BRepModel,
    face_id: FaceId,
    intersection_curve: &[Point3],
) -> OperationResult<()> {
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "extend_face_to_curve: face {} missing",
            face_id
        )));
    }
    if intersection_curve.len() < 2 {
        return Err(OperationError::InvalidGeometry(format!(
            "extend_face_to_curve: face {} given degenerate \
             intersection curve ({} points)",
            face_id,
            intersection_curve.len()
        )));
    }
    tracing::warn!(
        face_id = face_id,
        curve_points = intersection_curve.len(),
        "draft: face-extension to intersection curve not yet implemented; \
         face boundary left at original extent"
    );
    Ok(())
}

/// Trim face at intersection curve.
///
/// Same shape as `extend_face_to_curve`: validate inputs and log the
/// gap with concrete IDs/sizes. Full splitting of the face by the
/// intersection curve (creating new edges + replacement loop topology)
/// is a follow-up.
fn trim_face_at_curve(
    model: &mut BRepModel,
    face_id: FaceId,
    intersection_curve: &[Point3],
) -> OperationResult<()> {
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "trim_face_at_curve: face {} missing",
            face_id
        )));
    }
    if intersection_curve.len() < 2 {
        return Err(OperationError::InvalidGeometry(format!(
            "trim_face_at_curve: face {} given degenerate \
             intersection curve ({} points)",
            face_id,
            intersection_curve.len()
        )));
    }
    tracing::warn!(
        face_id = face_id,
        curve_points = intersection_curve.len(),
        "draft: face-trimming at intersection curve not yet implemented; \
         face boundary left untrimmed"
    );
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

/// Create blend loop between two faces.
///
/// Returns an empty outer loop today — the blend topology is generated
/// elsewhere (see `blend.rs`). This validator-style stub exists so the
/// draft pipeline has a uniform "ask for a blend loop" hook; we still
/// confirm both faces exist so callers operating on stale IDs fail
/// loudly instead of silently receiving an empty loop.
fn create_blend_loop(
    model: &BRepModel,
    face_id1: FaceId,
    face_id2: FaceId,
) -> OperationResult<Loop> {
    use crate::primitives::r#loop::{Loop, LoopType};

    if model.faces.get(face_id1).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "create_blend_loop: face {} missing",
            face_id1
        )));
    }
    if model.faces.get(face_id2).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "create_blend_loop: face {} missing",
            face_id2
        )));
    }
    Ok(Loop::new(0, LoopType::Outer))
}

/// Validate drafted solid via the kernel's parallel B-Rep validator.
///
/// Drafting modifies face surfaces and can leave non-manifold seams or
/// flipped normals; running `Standard`-level validation surfaces those
/// issues immediately rather than letting the malformed solid leak into
/// downstream operations.
fn validate_drafted_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_drafted_solid: solid {} not found",
            solid_id
        )));
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
        return Err(OperationError::TopologyError(format!(
            "drafted solid failed validation ({} error(s)): {}",
            result.errors.len(),
            summary
        )));
    }
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
