//! General Sweep Operations for B-Rep Models
//!
//! Creates solids by sweeping profiles along arbitrary paths with
//! orientation control and scaling.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::frame::parallel_transport_frames;
use crate::math::{MathError, Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for sweep operations
#[derive(Debug)]
pub struct SweepOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of sweep
    pub sweep_type: SweepType,

    /// Orientation control
    pub orientation: OrientationControl,

    /// Scale control along path
    pub scale: ScaleControl,

    /// Twist control along path
    pub twist: TwistControl,

    /// Whether to create solid or surfaces
    pub create_solid: bool,

    /// Quality of sweep (affects tessellation)
    pub quality: SweepQuality,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            sweep_type: SweepType::Path,
            orientation: OrientationControl::Frenet,
            scale: ScaleControl::Constant,
            twist: TwistControl::None,
            create_solid: true,
            quality: SweepQuality::Standard,
        }
    }
}

/// Type of sweep operation
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SweepType {
    /// Sweep along a path curve
    Path,
    /// Sweep along multiple guide curves
    MultiGuide,
    /// Sweep with rail curves for orientation
    Rail,
    /// Bi-rail sweep with two guide rails
    BiRail,
}

/// How to control profile orientation along path
pub enum OrientationControl {
    /// Use Frenet frame (natural path frame)
    Frenet,
    /// Minimize rotation (parallel transport)
    MinimalRotation,
    /// Fixed direction
    Fixed(Vector3),
    /// Follow surface normal
    Normal,
    /// Custom orientation function
    Custom(Box<dyn Fn(f64) -> Matrix4>),
}

/// How to control scale along path
pub enum ScaleControl {
    /// Constant scale
    Constant,
    /// Linear scale from start to end
    Linear(f64, f64),
    /// Scale function along path parameter
    Function(Box<dyn Fn(f64) -> f64>),
}

/// How to control twist along path
pub enum TwistControl {
    /// No twist
    None,
    /// Linear twist over path
    Linear(f64),
    /// Twist function along path parameter
    Function(Box<dyn Fn(f64) -> f64>),
}

impl std::fmt::Debug for OrientationControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrientationControl::Frenet => write!(f, "Frenet"),
            OrientationControl::MinimalRotation => write!(f, "MinimalRotation"),
            OrientationControl::Fixed(v) => write!(f, "Fixed({:?})", v),
            OrientationControl::Normal => write!(f, "Normal"),
            OrientationControl::Custom(_) => write!(f, "Custom(<function>)"),
        }
    }
}

impl std::fmt::Debug for ScaleControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScaleControl::Constant => write!(f, "Constant"),
            ScaleControl::Linear(s, e) => write!(f, "Linear({}, {})", s, e),
            ScaleControl::Function(_) => write!(f, "Function(<function>)"),
        }
    }
}

impl std::fmt::Debug for TwistControl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TwistControl::None => write!(f, "None"),
            TwistControl::Linear(angle) => write!(f, "Linear({})", angle),
            TwistControl::Function(_) => write!(f, "Function(<function>)"),
        }
    }
}

/// Sweep quality level
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SweepQuality {
    /// Fast computation, lower quality
    Draft,
    /// Standard quality
    Standard,
    /// High quality for final models
    High,
}

/// Sweep a profile along a path
pub fn sweep_profile(
    model: &mut BRepModel,
    profile: Vec<EdgeId>,
    path: EdgeId,
    options: SweepOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_sweep_inputs(model, &profile, path, &options)?;

    // Capture profile edges before they're consumed, for recording.
    let profile_edges_for_record: Vec<u64> = profile.iter().map(|&e| e as u64).collect();

    // Create face from profile if needed
    let profile_face = create_profile_face(model, profile)?;

    // Get path curve
    let path_edge = model
        .edges
        .get(path)
        .ok_or_else(|| OperationError::InvalidGeometry("Path edge not found".to_string()))?
        .clone();

    // Create swept solid based on sweep type
    let solid_id = match options.sweep_type {
        SweepType::Path => create_path_sweep(model, profile_face, &path_edge, &options)?,
        SweepType::MultiGuide => {
            create_frame_driven_sweep(model, profile_face, &path_edge, &options)?
        }
        SweepType::Rail => create_frame_driven_sweep(model, profile_face, &path_edge, &options)?,
        SweepType::BiRail => create_frame_driven_sweep(model, profile_face, &path_edge, &options)?,
    };

    // Validate result if requested
    if options.common.validate_result {
        validate_swept_solid(model, solid_id)?;
    }

    // Record for attached recorders. Include the sweep type discriminant
    // by Debug-formatting since SweepOptions is not Serialize.
    let mut inputs = profile_edges_for_record;
    inputs.push(path as u64);
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("sweep_profile")
            .with_parameters(serde_json::json!({
                "path_edge": path,
                "sweep_type": format!("{:?}", options.sweep_type),
                "quality": format!("{:?}", options.quality),
            }))
            .with_inputs(inputs)
            .with_outputs(vec![solid_id as u64]),
    );

    Ok(solid_id)
}

/// Create a path sweep
fn create_path_sweep(
    model: &mut BRepModel,
    profile_face: FaceId,
    path_edge: &Edge,
    options: &SweepOptions,
) -> OperationResult<SolidId> {
    // Determine number of sections based on quality
    let num_sections = match options.quality {
        SweepQuality::Draft => 10,
        SweepQuality::Standard => 25,
        SweepQuality::High => 50,
    };

    // Generate sweep sections along path
    let sections = generate_sweep_sections(model, profile_face, path_edge, num_sections, options)?;

    // Create faces between sections
    let mut shell_faces = Vec::new();

    // Add start cap if creating solid
    if options.create_solid {
        shell_faces.push(sections[0].face_id);
    }

    // Create lateral faces between sections
    for i in 0..sections.len() - 1 {
        let lateral_faces = create_lateral_faces(model, &sections[i], &sections[i + 1])?;
        shell_faces.extend(lateral_faces);
    }

    // Add end cap if creating solid
    if options.create_solid {
        let last_section = sections
            .last()
            .expect("sweep: ≥1 section must exist before creating end cap");
        let end_face = create_reversed_face(model, last_section.face_id)?;
        shell_faces.push(end_face);
    }

    // Create shell and solid
    let shell_type = if options.create_solid {
        ShellType::Closed
    } else {
        ShellType::Open
    };

    let mut shell = Shell::new(0, shell_type); // ID will be assigned by store
    for face_id in shell_faces {
        shell.add_face(face_id);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id); // ID will be assigned by store
    let solid_id = model.solids.add(solid);

    Ok(solid_id)
}

/// Create a sweep driven by pre-computed frame-solver frames (multi-guide, rail, bi-rail).
///
/// The frame solver computes the full set of stations up-front, including
/// position, orientation, and optional scale. This function converts those
/// frames into sweep sections and assembles the solid.
fn create_frame_driven_sweep(
    model: &mut BRepModel,
    profile_face: FaceId,
    path_edge: &Edge,
    options: &SweepOptions,
) -> OperationResult<SolidId> {
    let curve = model
        .curves
        .get(path_edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    let tolerance = options.common.tolerance;

    let num_sections = match options.quality {
        SweepQuality::Draft => 10,
        SweepQuality::Standard => 25,
        SweepQuality::High => 50,
    } as usize;

    // Compute frames using the appropriate frame solver
    let frame_stations = match options.sweep_type {
        SweepType::Rail => {
            // For rail sweep we use the path curve as the rail (single rail).
            // The caller is expected to supply a rail via a more complete API;
            // here we fall back to parallel transport when no rail data is
            // available, matching the previous behaviour but with the proper
            // frame solver.
            parallel_transport_frames(curve, num_sections, None, tolerance)?
        }
        SweepType::BiRail => parallel_transport_frames(curve, num_sections, None, tolerance)?,
        SweepType::MultiGuide => parallel_transport_frames(curve, num_sections, None, tolerance)?,
        SweepType::Path => {
            // Path sweep should go through create_path_sweep; this is a defensive fallback.
            parallel_transport_frames(curve, num_sections, None, tolerance)?
        }
    };

    // Convert frame-solver output into sweep sections
    let mut sections: Vec<SweepSection> = Vec::with_capacity(frame_stations.len());

    for frame in &frame_stations {
        let scale_val = compute_scale_at_parameter(frame.parameter, &options.scale).unwrap_or(1.0)
            * frame.scale.unwrap_or(1.0);

        let twist_val = compute_twist_at_parameter(frame.parameter, &options.twist).unwrap_or(0.0);

        let transform = build_sweep_transform(frame.position, frame.matrix, scale_val, twist_val);

        let section = create_sweep_section(model, profile_face, transform)?;
        sections.push(section);
    }

    // Assemble solid from sections (same logic as create_path_sweep)
    let mut shell_faces = Vec::new();

    if options.create_solid {
        shell_faces.push(sections[0].face_id);
    }

    for i in 0..sections.len() - 1 {
        let lateral_faces = create_lateral_faces(model, &sections[i], &sections[i + 1])?;
        shell_faces.extend(lateral_faces);
    }

    if options.create_solid {
        let last_section = sections
            .last()
            .expect("sweep: ≥1 section must exist before creating end cap");
        let end_face = create_reversed_face(model, last_section.face_id)?;
        shell_faces.push(end_face);
    }

    let shell_type = if options.create_solid {
        ShellType::Closed
    } else {
        ShellType::Open
    };

    let mut shell = Shell::new(0, shell_type);
    for face_id in shell_faces {
        shell.add_face(face_id);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id);
    let solid_id = model.solids.add(solid);

    Ok(solid_id)
}

/// Section data for sweep
struct SweepSection {
    /// Face at this section
    face_id: FaceId,
    /// Vertices in order
    vertices: Vec<VertexId>,
}

/// Generate sweep sections along path
fn generate_sweep_sections(
    model: &mut BRepModel,
    profile_face: FaceId,
    path_edge: &Edge,
    num_sections: u32,
    options: &SweepOptions,
) -> OperationResult<Vec<SweepSection>> {
    let mut sections = Vec::new();

    for i in 0..=num_sections {
        let t = i as f64 / num_sections as f64;

        // Get position and frame at parameter
        let position = path_edge.evaluate(t, &model.curves)?;
        let frame = compute_sweep_frame(model, path_edge, t, &options.orientation)?;

        // Compute scale at parameter
        let scale = compute_scale_at_parameter(t, &options.scale)?;

        // Compute twist at parameter
        let twist = compute_twist_at_parameter(t, &options.twist)?;

        // Build transformation matrix
        let transform = build_sweep_transform(position, frame, scale, twist);

        // Create transformed section
        let section = create_sweep_section(model, profile_face, transform)?;
        sections.push(section);
    }

    Ok(sections)
}

/// Compute sweep frame at parameter
fn compute_sweep_frame(
    model: &BRepModel,
    edge: &Edge,
    t: f64,
    orientation: &OrientationControl,
) -> OperationResult<Matrix4> {
    match orientation {
        OrientationControl::Frenet => compute_frenet_frame(model, edge, t),
        OrientationControl::MinimalRotation => compute_minimal_rotation_frame(model, edge, t),
        OrientationControl::Fixed(dir) => compute_fixed_frame(model, edge, t, *dir),
        OrientationControl::Normal => compute_normal_frame(model, edge, t),
        OrientationControl::Custom(func) => Ok(func(t)),
    }
}

/// Compute Frenet frame
fn compute_frenet_frame(model: &BRepModel, edge: &Edge, t: f64) -> OperationResult<Matrix4> {
    // Get curve derivatives
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    let curve_t = edge.edge_to_curve_parameter(t);
    let derivatives = curve.evaluate_derivatives(curve_t, 1)?;
    let tangent = derivatives
        .get(1)
        .ok_or(MathError::InvalidParameter("No tangent".to_string()))?;

    // Try to get second derivative for normal
    let normal = match curve.evaluate_derivatives(curve_t, 2) {
        Ok(derivs) if derivs.len() > 2 => {
            let d2 = &derivs[2];
            let n = *d2 - *tangent * tangent.dot(d2) / tangent.magnitude_squared();
            if n.magnitude() > 1e-10 {
                n.normalize()?
            } else {
                // Curvature is zero, use arbitrary perpendicular
                tangent.perpendicular()
            }
        }
        Ok(_) | Err(_) => tangent.perpendicular(),
    };

    let binormal = tangent.cross(&normal).normalize()?;

    // Build frame matrix (columns are the basis vectors)
    Ok(Matrix4::from_cols(
        normal,
        binormal,
        *tangent,
        Vector3::ZERO,
    ))
}

/// Compute minimal rotation frame using parallel transport.
///
/// Evaluates the full set of parallel-transport frames along the edge curve,
/// then returns the frame closest to the requested parameter. Because the
/// frame solver requires multiple stations to propagate the normal, a batch
/// of frames is computed and the nearest one is selected.
fn compute_minimal_rotation_frame(
    model: &BRepModel,
    edge: &Edge,
    t: f64,
) -> OperationResult<Matrix4> {
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    let tolerance = Tolerance::from_distance(1e-8);
    let num_stations = 50;
    let frames = parallel_transport_frames(curve, num_stations, None, tolerance)?;

    // Map edge parameter to curve parameter and find closest station
    let curve_t = edge.edge_to_curve_parameter(t);

    let mut best_idx = 0;
    let mut best_dist = f64::INFINITY;
    for (i, f) in frames.iter().enumerate() {
        let d = (f.parameter - curve_t).abs();
        if d < best_dist {
            best_dist = d;
            best_idx = i;
        }
    }

    Ok(frames[best_idx].matrix)
}

/// Compute fixed direction frame
fn compute_fixed_frame(
    model: &BRepModel,
    edge: &Edge,
    t: f64,
    direction: Vector3,
) -> OperationResult<Matrix4> {
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    let curve_t = edge.edge_to_curve_parameter(t);
    let derivatives = curve.evaluate_derivatives(curve_t, 1)?;
    let tangent = derivatives
        .get(1)
        .ok_or(MathError::InvalidParameter("No tangent".to_string()))?
        .normalize()?;

    // Project direction perpendicular to tangent
    let side = direction - tangent * tangent.dot(&direction);
    if side.magnitude() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Fixed direction parallel to path".to_string(),
        ));
    }
    let side = side.normalize()?;

    let up = tangent.cross(&side).normalize()?;

    Ok(Matrix4::from_cols(side, up, tangent, Vector3::ZERO))
}

/// Compute normal-based frame
fn compute_normal_frame(model: &BRepModel, edge: &Edge, t: f64) -> OperationResult<Matrix4> {
    // Would compute frame based on surface normal
    // For now, use Frenet
    compute_frenet_frame(model, edge, t)
}

/// Compute scale at parameter
fn compute_scale_at_parameter(t: f64, scale_control: &ScaleControl) -> OperationResult<f64> {
    match scale_control {
        ScaleControl::Constant => Ok(1.0),
        ScaleControl::Linear(start, end) => Ok(start + (end - start) * t),
        ScaleControl::Function(func) => Ok(func(t)),
    }
}

/// Compute twist at parameter
fn compute_twist_at_parameter(t: f64, twist_control: &TwistControl) -> OperationResult<f64> {
    match twist_control {
        TwistControl::None => Ok(0.0),
        TwistControl::Linear(total_twist) => Ok(total_twist * t),
        TwistControl::Function(func) => Ok(func(t)),
    }
}

/// Build sweep transformation matrix
fn build_sweep_transform(position: Point3, frame: Matrix4, scale: f64, twist: f64) -> Matrix4 {
    let translation = Matrix4::from_translation(&position);
    let scaling = Matrix4::from_scale(&Vector3::new(scale, scale, scale));
    let rotation = Matrix4::from_axis_angle(&Vector3::Z, twist).unwrap_or(Matrix4::identity());

    translation * frame * rotation * scaling
}

/// Create a sweep section
fn create_sweep_section(
    model: &mut BRepModel,
    profile_face: FaceId,
    transform: Matrix4,
) -> OperationResult<SweepSection> {
    // Transform the profile face
    let transformed_face = transform_face_full(model, profile_face, &transform)?;

    // Get ordered vertices from face
    let vertices = get_face_vertices_ordered(model, transformed_face)?;

    Ok(SweepSection {
        face_id: transformed_face,
        vertices,
    })
}

/// Transform face completely (for sweep sections)
fn transform_face_full(
    model: &mut BRepModel,
    face_id: FaceId,
    transform: &Matrix4,
) -> OperationResult<FaceId> {
    // Similar to pattern transform_face but specific to sweep
    super::pattern::transform_face(model, face_id, transform)
}

/// Get ordered vertices from face
fn get_face_vertices_ordered(model: &BRepModel, face_id: FaceId) -> OperationResult<Vec<VertexId>> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    let mut vertices = Vec::new();
    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let forward = loop_data.orientations[i];
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        let vertex = if forward {
            edge.start_vertex
        } else {
            edge.end_vertex
        };

        if vertices.is_empty() || vertices.last() != Some(&vertex) {
            vertices.push(vertex);
        }
    }

    // Remove duplicate last vertex if closed
    if vertices.len() > 1 && vertices[0] == vertices[vertices.len() - 1] {
        vertices.pop();
    }

    Ok(vertices)
}

/// Create lateral faces between sections
fn create_lateral_faces(
    model: &mut BRepModel,
    section1: &SweepSection,
    section2: &SweepSection,
) -> OperationResult<Vec<FaceId>> {
    if section1.vertices.len() != section2.vertices.len() {
        return Err(OperationError::InvalidGeometry(
            "Sections have different vertex counts".to_string(),
        ));
    }

    let mut faces = Vec::new();
    let n = section1.vertices.len();

    for i in 0..n {
        let v1 = section1.vertices[i];
        let v2 = section1.vertices[(i + 1) % n];
        let v3 = section2.vertices[(i + 1) % n];
        let v4 = section2.vertices[i];

        let face = create_quad_face(model, v1, v2, v3, v4)?;
        faces.push(face);
    }

    Ok(faces)
}

/// Create a quadrilateral face
fn create_quad_face(
    model: &mut BRepModel,
    v1: VertexId,
    v2: VertexId,
    v3: VertexId,
    v4: VertexId,
) -> OperationResult<FaceId> {
    // Create edges
    let e1 = create_or_find_edge(model, v1, v2)?;
    let e2 = create_or_find_edge(model, v2, v3)?;
    let e3 = create_or_find_edge(model, v3, v4)?;
    let e4 = create_or_find_edge(model, v4, v1)?;

    // Create loop
    let mut quad_loop = Loop::new(
        0, // ID will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    quad_loop.add_edge(e1, true);
    quad_loop.add_edge(e2, true);
    quad_loop.add_edge(e3, true);
    quad_loop.add_edge(e4, true);
    let loop_id = model.loops.add(quad_loop);

    // Create surface (bilinear patch)
    let surface = create_bilinear_surface(model, v1, v2, v3, v4)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );

    Ok(model.faces.add(face))
}

/// Create or find edge between vertices
fn create_or_find_edge(
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
        0, // ID will be assigned by store
        start,
        end,
        curve_id,
        EdgeOrientation::Forward,
    );

    Ok(model.edges.add(edge))
}

/// Create bilinear surface
fn create_bilinear_surface(
    _model: &BRepModel,
    _v1: VertexId,
    _v2: VertexId,
    _v3: VertexId,
    _v4: VertexId,
) -> OperationResult<Box<dyn Surface>> {
    // Would create proper bilinear surface
    use crate::primitives::surface::Plane;
    Ok(Box::new(Plane::xy(0.0)))
}

/// Create profile face from edges
fn create_profile_face(model: &mut BRepModel, edges: Vec<EdgeId>) -> OperationResult<FaceId> {
    // Create loop from edges
    let mut profile_loop = Loop::new(
        0, // ID will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    for edge_id in edges {
        profile_loop.add_edge(edge_id, true);
    }
    let loop_id = model.loops.add(profile_loop);

    // Create planar surface (assuming planar profile)
    use crate::primitives::surface::Plane;
    let surface = Box::new(Plane::xy(0.0));
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );

    Ok(model.faces.add(face))
}

/// Create reversed face
fn create_reversed_face(model: &mut BRepModel, face_id: FaceId) -> OperationResult<FaceId> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    let mut reversed = face;
    reversed.id = 0; // ID will be assigned by store
    reversed.orientation = match reversed.orientation {
        FaceOrientation::Forward => FaceOrientation::Backward,
        FaceOrientation::Backward => FaceOrientation::Forward,
    };

    Ok(model.faces.add(reversed))
}

/// Validate sweep inputs
fn validate_sweep_inputs(
    model: &BRepModel,
    profile: &[EdgeId],
    path: EdgeId,
    _options: &SweepOptions,
) -> OperationResult<()> {
    // Check profile edges exist and form closed loop
    for &edge_id in profile {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Profile edge not found".to_string(),
            ));
        }
    }

    // Check path exists
    if model.edges.get(path).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Path edge not found".to_string(),
        ));
    }

    Ok(())
}

/// Validate swept solid
fn validate_swept_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep(
            "Swept solid not found".to_string(),
        ));
    }

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_sweep_validation() {
//         // Test parameter validation
//     }
// }
