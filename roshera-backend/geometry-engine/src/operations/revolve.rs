//! Revolution/Sweep Operations for B-Rep Models
//!
//! Creates solids of revolution by rotating profiles around an axis.
//! Supports partial revolutions, multiple profiles, and helical paths.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    curve::ParameterRange,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for revolution operations
#[derive(Debug, Clone)]
pub struct RevolveOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Axis origin point
    pub axis_origin: Point3,

    /// Axis direction (will be normalized)
    pub axis_direction: Vector3,

    /// Revolution angle in radians (2π for full revolution)
    pub angle: f64,

    /// Whether revolution is symmetric (extends in both directions from axis)
    pub symmetric: bool,

    /// Number of segments for discretization
    pub segments: u32,

    /// Helical pitch (0 for pure rotation)
    pub pitch: f64,

    /// Whether to create end caps for partial revolutions
    pub cap_ends: bool,
}

impl Default for RevolveOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            symmetric: false,
            segments: 32,
            pitch: 0.0,
            cap_ends: true,
        }
    }
}

/// Revolve a face around an axis to create a solid
pub fn revolve_face(
    model: &mut BRepModel,
    face_id: FaceId,
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_revolve_inputs(model, face_id, &options)?;

    // Normalize axis direction
    let axis_dir = options.axis_direction.normalize()?;

    // Get the face to revolve
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Check if face intersects the axis (would create self-intersection)
    if face_intersects_axis(model, &face, options.axis_origin, axis_dir)? {
        return Err(OperationError::SelfIntersection);
    }

    // Create revolved solid
    let solid_id = if options.pitch.abs() < 1e-10 {
        // Pure revolution
        create_revolution(model, &face, face_id, &options)?
    } else {
        // Helical sweep
        create_helical_sweep(model, &face, face_id, &options)?
    };

    // Validate result if requested
    if options.common.validate_result {
        validate_revolved_solid(model, solid_id)?;
    }

    // Record for attached recorders.
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("revolve_face")
            .with_parameters(serde_json::json!({
                "face_id": face_id,
                "axis_origin": [
                    options.axis_origin.x,
                    options.axis_origin.y,
                    options.axis_origin.z,
                ],
                "axis_direction": [
                    options.axis_direction.x,
                    options.axis_direction.y,
                    options.axis_direction.z,
                ],
                "angle": options.angle,
                "pitch": options.pitch,
                "segments": options.segments,
                "cap_ends": options.cap_ends,
            }))
            .with_inputs(vec![face_id as u64])
            .with_outputs(vec![solid_id as u64]),
    );

    Ok(solid_id)
}

/// Revolve a wire/profile to create a solid
pub fn revolve_profile(
    model: &mut BRepModel,
    profile_edges: Vec<EdgeId>,
    options: RevolveOptions,
) -> OperationResult<SolidId> {
    // First create a face from the profile
    let face_id = create_face_from_profile(model, profile_edges)?;

    // Then revolve the face
    revolve_face(model, face_id, options)
}

/// Create a pure revolution (no helical component)
fn create_revolution(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<SolidId> {
    let mut shell_faces = Vec::new();
    let is_full_revolution = (options.angle - std::f64::consts::TAU).abs() < 1e-10;

    // Create revolved surfaces for each edge of the face
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    // For each edge in the profile, create a surface of revolution
    for &edge_id in &base_loop.edges {
        let surface_faces = create_revolved_edge_surface(
            model,
            edge_id,
            true, // Assuming forward orientation
            options.axis_origin,
            options.axis_direction,
            options.angle,
            options.segments,
        )?;
        shell_faces.extend(surface_faces);
    }

    // Add end caps for partial revolutions
    if !is_full_revolution && options.cap_ends {
        // Start cap (original face)
        shell_faces.push(base_face_id);

        // End cap (rotated face)
        let end_rotation = Matrix4::from_axis_angle(&options.axis_direction, options.angle)?;
        let end_face = create_transformed_face(model, base_face, end_rotation)?;
        shell_faces.push(end_face);
    }

    // Create shell and solid
    let shell_type = if is_full_revolution || options.cap_ends {
        ShellType::Closed
    } else {
        ShellType::Open
    };

    let mut shell = Shell::new(0, shell_type); // ID will be assigned by store
    for face_id in &shell_faces {
        shell.add_face(*face_id);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id); // ID will be assigned by store
    let solid_id = model.solids.add(solid);

    Ok(solid_id)
}

/// Create a helical sweep — revolve with axial translation (pitch per revolution)
fn create_helical_sweep(
    model: &mut BRepModel,
    base_face: &Face,
    _base_face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<SolidId> {
    let segments = options.segments.max(4);
    let angle_step = options.angle / segments as f64;
    // Axial translation per angle step
    let pitch_step = options.pitch * (angle_step / (2.0 * std::f64::consts::PI));

    let outer_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Face loop not found".into()))?
        .clone();

    let mut shell_faces = Vec::new();

    // Generate faces for each segment by composing rotation + translation
    for seg in 0..segments {
        let angle = angle_step * seg as f64;
        let next_angle = angle_step * (seg + 1) as f64;
        let axial_offset = pitch_step * seg as f64;
        let next_axial = pitch_step * (seg + 1) as f64;

        // Build combined transforms: rotate then translate along axis
        let rot = Matrix4::from_axis_angle(&options.axis_direction, angle)?;
        let next_rot = Matrix4::from_axis_angle(&options.axis_direction, next_angle)?;
        let translate = Matrix4::from_translation(&(options.axis_direction * axial_offset));
        let next_translate = Matrix4::from_translation(&(options.axis_direction * next_axial));
        let xform = translate * rot;
        let next_xform = next_translate * next_rot;

        // Create faces for each edge in the profile loop. The loop index
        // `i` is folded into error messages so revolve failures point to a
        // specific profile edge rather than the abstract "edge not found".
        for (i, &edge_id) in outer_loop.edges.iter().enumerate() {
            let edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry(format!(
                    "revolve: edge {} (profile slot {}) not found",
                    edge_id, i
                )))?
                .clone();

            // Get edge endpoints and transform them
            let ps_arr = model
                .vertices
                .get_position(edge.start_vertex)
                .ok_or_else(|| OperationError::InvalidGeometry(format!(
                    "revolve: start vertex {} of edge {} (profile slot {}) not found",
                    edge.start_vertex, edge_id, i
                )))?;
            let pe_arr = model
                .vertices
                .get_position(edge.end_vertex)
                .ok_or_else(|| OperationError::InvalidGeometry(format!(
                    "revolve: end vertex {} of edge {} (profile slot {}) not found",
                    edge.end_vertex, edge_id, i
                )))?;
            let p_start = Vector3::new(ps_arr[0], ps_arr[1], ps_arr[2]);
            let p_end = Vector3::new(pe_arr[0], pe_arr[1], pe_arr[2]);

            let p1 = xform.transform_point(&p_start);
            let p2 = xform.transform_point(&p_end);
            let p3 = next_xform.transform_point(&p_end);
            let p4 = next_xform.transform_point(&p_start);

            // Create quad face from these 4 points
            let v1 = model.vertices.add(p1.x, p1.y, p1.z);
            let v2 = model.vertices.add(p2.x, p2.y, p2.z);
            let v3 = model.vertices.add(p3.x, p3.y, p3.z);
            let v4 = model.vertices.add(p4.x, p4.y, p4.z);

            use crate::primitives::curve::Line;
            use crate::primitives::edge::EdgeOrientation;
            use crate::primitives::face::FaceOrientation;
            use crate::primitives::r#loop::LoopType;
            use crate::primitives::surface::Plane;

            let l1 = model.curves.add(Box::new(Line::new(p1, p2)));
            let l2 = model.curves.add(Box::new(Line::new(p2, p3)));
            let l3 = model.curves.add(Box::new(Line::new(p3, p4)));
            let l4 = model.curves.add(Box::new(Line::new(p4, p1)));

            let e1 = model.edges.add(Edge::new_auto_range(
                0,
                v1,
                v2,
                l1,
                EdgeOrientation::Forward,
            ));
            let e2 = model.edges.add(Edge::new_auto_range(
                0,
                v2,
                v3,
                l2,
                EdgeOrientation::Forward,
            ));
            let e3 = model.edges.add(Edge::new_auto_range(
                0,
                v3,
                v4,
                l3,
                EdgeOrientation::Forward,
            ));
            let e4 = model.edges.add(Edge::new_auto_range(
                0,
                v4,
                v1,
                l4,
                EdgeOrientation::Forward,
            ));

            let mut face_loop = Loop::new(0, LoopType::Outer);
            face_loop.add_edge(e1, true);
            face_loop.add_edge(e2, true);
            face_loop.add_edge(e3, true);
            face_loop.add_edge(e4, true);
            let loop_id = model.loops.add(face_loop);

            // Create planar surface from the quad normal
            let n = (p2 - p1).cross(&(p4 - p1));
            let normal = if n.magnitude_squared() > 1e-20 {
                n.normalize()?
            } else {
                Vector3::Z
            };
            let surf = Plane::from_point_normal(p1, normal)?;
            let surf_id = model.surfaces.add(Box::new(surf));

            let face = Face::new(0, surf_id, loop_id, FaceOrientation::Forward);
            shell_faces.push(model.faces.add(face));
        }
    }

    // Build shell and solid
    let shell_type = if options.cap_ends {
        ShellType::Closed
    } else {
        ShellType::Open
    };
    let mut shell = Shell::new(0, shell_type);
    for &fid in &shell_faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);
    let solid = Solid::new(0, shell_id);
    Ok(model.solids.add(solid))
}

/// Create surface(s) by revolving an edge
fn create_revolved_edge_surface(
    model: &mut BRepModel,
    edge_id: EdgeId,
    edge_forward: bool,
    axis_origin: Point3,
    axis_direction: Vector3,
    angle: f64,
    segments: u32,
) -> OperationResult<Vec<FaceId>> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    let mut faces = Vec::new();
    let segment_angle = angle / segments as f64;

    // Create faces for each segment
    for i in 0..segments {
        let start_angle = i as f64 * segment_angle;
        let end_angle = (i + 1) as f64 * segment_angle;

        let face_id = create_revolution_segment_face(
            model,
            &edge,
            edge_forward,
            axis_origin,
            axis_direction,
            start_angle,
            end_angle,
        )?;
        faces.push(face_id);
    }

    Ok(faces)
}

/// Create a single face for a revolution segment
fn create_revolution_segment_face(
    model: &mut BRepModel,
    edge: &Edge,
    edge_forward: bool,
    axis_origin: Point3,
    axis_direction: Vector3,
    start_angle: f64,
    end_angle: f64,
) -> OperationResult<FaceId> {
    // Get edge endpoints
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    // Create rotated vertices
    let rotation_start = Matrix4::from_axis_angle(&axis_direction, start_angle)?;
    let rotation_end = Matrix4::from_axis_angle(&axis_direction, end_angle)?;

    let _v0 = edge.start_vertex;
    let _v1 = edge.end_vertex;
    let v2 = create_rotated_vertex(model, &end_vertex, axis_origin, rotation_start)?;
    let v3 = create_rotated_vertex(model, &end_vertex, axis_origin, rotation_end)?;
    let v4 = create_rotated_vertex(model, &start_vertex, axis_origin, rotation_end)?;
    let v5 = create_rotated_vertex(model, &start_vertex, axis_origin, rotation_start)?;

    // Create edges for the face
    let mut face_edges = Vec::new();

    // Edge 1: Original edge at start angle (or rotated copy if not at 0)
    if start_angle.abs() < 1e-10 {
        face_edges.push((edge.id, edge_forward));
    } else {
        let rotated_edge = create_rotated_edge(model, edge, axis_origin, rotation_start)?;
        face_edges.push((rotated_edge, edge_forward));
    }

    // Edge 2: Meridian from end of profile edge
    let meridian1 = create_meridian_edge(
        model,
        v2,
        v3,
        axis_origin,
        axis_direction,
        start_angle,
        end_angle,
    )?;
    face_edges.push((meridian1, true));

    // Edge 3: Rotated edge at end angle (reversed)
    let rotated_edge_end = create_rotated_edge(model, edge, axis_origin, rotation_end)?;
    face_edges.push((rotated_edge_end, !edge_forward));

    // Edge 4: Meridian from start of profile edge (reversed)
    let meridian2 = create_meridian_edge(
        model,
        v5,
        v4,
        axis_origin,
        axis_direction,
        start_angle,
        end_angle,
    )?;
    face_edges.push((meridian2, false));

    // Create loop
    let mut face_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer); // ID will be assigned by store
    for (edge_id, forward) in face_edges {
        face_loop.add_edge(edge_id, forward);
    }
    let loop_id = model.loops.add(face_loop);

    // Create surface of revolution
    let surface = create_revolution_surface(model, edge.curve_id, axis_origin, axis_direction)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );
    let face_id = model.faces.add(face);

    Ok(face_id)
}

/// Create a rotated vertex
fn create_rotated_vertex(
    model: &mut BRepModel,
    vertex: &crate::primitives::vertex::Vertex,
    axis_origin: Point3,
    rotation: Matrix4,
) -> OperationResult<VertexId> {
    let pos = Vector3::from(vertex.position);
    let relative_pos = pos - axis_origin;
    let rotated_pos = rotation.transform_point(&relative_pos) + axis_origin;

    Ok(model
        .vertices
        .add(rotated_pos.x, rotated_pos.y, rotated_pos.z))
}

/// Create a meridian edge (arc on surface of revolution)
fn create_meridian_edge(
    model: &mut BRepModel,
    start_vertex: VertexId,
    end_vertex: VertexId,
    axis_origin: Point3,
    axis_direction: Vector3,
    start_angle: f64,
    end_angle: f64,
) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Arc;

    // Get vertex position
    let vertex_pos = model
        .vertices
        .get(start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?
        .position;
    let point = Vector3::from(vertex_pos);

    // Project point to plane perpendicular to axis
    let to_point = point - axis_origin;
    let axis_component = to_point.dot(&axis_direction) * axis_direction;
    let radial_component = to_point - axis_component;
    let radius = radial_component.magnitude();

    if radius < 1e-10 {
        // Point is on axis, create degenerate edge
        return create_degenerate_edge(model, start_vertex, end_vertex);
    }

    // Create arc
    let center = axis_origin + axis_component;
    let arc = Arc::new(
        center,
        axis_direction,
        radius,
        start_angle,
        end_angle - start_angle,
    )?;
    let curve_id = model.curves.add(Box::new(arc));

    let edge = Edge::new_auto_range(
        0, // ID will be assigned by store
        start_vertex,
        end_vertex,
        curve_id,
        EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Create a rotated copy of an edge
fn create_rotated_edge(
    model: &mut BRepModel,
    edge: &Edge,
    axis_origin: Point3,
    rotation: Matrix4,
) -> OperationResult<EdgeId> {
    // Get original curve
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Create transformation that rotates around axis
    let to_origin = Matrix4::from_translation(&-axis_origin);
    let from_origin = Matrix4::from_translation(&axis_origin);
    let transform = from_origin * rotation * to_origin;

    // Create transformed curve
    let rotated_curve = curve.transform(&transform);
    let new_curve_id = model.curves.add(rotated_curve);

    // Get rotated vertices
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let new_start = create_rotated_vertex(model, &start_vertex, axis_origin, rotation)?;
    let new_end = create_rotated_vertex(model, &end_vertex, axis_origin, rotation)?;

    // Create new edge
    let new_edge = Edge::new(
        0, // ID will be assigned by store
        new_start,
        new_end,
        new_curve_id,
        edge.orientation,
        edge.param_range,
    );
    let edge_id = model.edges.add(new_edge);

    Ok(edge_id)
}

/// Create a surface of revolution from a profile curve rotated around an axis.
fn create_revolution_surface(
    model: &mut BRepModel,
    profile_curve_id: u32,
    axis_origin: Point3,
    axis_direction: Vector3,
) -> OperationResult<Box<dyn Surface>> {
    let curve = model
        .curves
        .get(profile_curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Profile curve not found".to_string()))?;

    let profile_clone = curve.clone_box();

    let revolution = crate::primitives::surface::SurfaceOfRevolution::new(
        axis_origin,
        axis_direction,
        profile_clone,
        std::f64::consts::TAU, // Full 360° revolution by default
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to create revolution surface: {e}"))
    })?;

    Ok(Box::new(revolution))
}

/// Create a transformed copy of a face.
///
/// Transforms the surface, creates new vertices/edges/loops for the boundary,
/// and produces a new face referencing the transformed geometry.
fn create_transformed_face(
    model: &mut BRepModel,
    face: &Face,
    transform: Matrix4,
) -> OperationResult<FaceId> {
    // Transform the surface
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
    let new_surface = surface.transform(&transform);
    let new_surface_id = model.surfaces.add(new_surface);

    // Transform the outer loop
    let outer_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Outer loop not found".to_string()))?
        .clone();

    let mut new_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);

    for (idx, &edge_id) in outer_loop.edges.iter().enumerate() {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
            .clone();

        // Transform curve
        let curve = model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
        let new_curve = curve.transform(&transform);
        let new_curve_id = model.curves.add(new_curve);

        // Transform vertices
        let sv = model
            .vertices
            .get(edge.start_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
        let ev = model
            .vertices
            .get(edge.end_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

        let new_start_pos =
            transform.transform_point(&Point3::new(sv.position[0], sv.position[1], sv.position[2]));
        let new_end_pos =
            transform.transform_point(&Point3::new(ev.position[0], ev.position[1], ev.position[2]));

        let new_start =
            model
                .vertices
                .add_or_find(new_start_pos.x, new_start_pos.y, new_start_pos.z, 1e-6);
        let new_end = model
            .vertices
            .add_or_find(new_end_pos.x, new_end_pos.y, new_end_pos.z, 1e-6);

        let new_edge = Edge::new(
            0,
            new_start,
            new_end,
            new_curve_id,
            edge.orientation,
            edge.param_range,
        );
        let new_edge_id = model.edges.add(new_edge);

        let forward = outer_loop.orientations.get(idx).copied().unwrap_or(true);
        new_loop.add_edge(new_edge_id, forward);
    }

    let new_loop_id = model.loops.add(new_loop);

    let new_face = Face::new(0, new_surface_id, new_loop_id, face.orientation);
    let new_face_id = model.faces.add(new_face);

    Ok(new_face_id)
}

/// Create a face from a closed wire profile
fn create_face_from_profile(
    model: &mut BRepModel,
    profile_edges: Vec<EdgeId>,
) -> OperationResult<FaceId> {
    // Reuse from extrude module
    super::extrude::create_face_from_profile(model, profile_edges)
}

/// Create a degenerate edge (point edge)
fn create_degenerate_edge(
    model: &mut BRepModel,
    vertex: VertexId,
    _end_vertex: VertexId,
) -> OperationResult<EdgeId> {
    let vertex_data = model
        .vertices
        .get(vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

    // Represent a point-edge with a zero-length Line (start == end). The kernel
    // does not maintain a dedicated Point curve type because every consumer of
    // a degenerate edge must also handle the zero-arc-length case on regular
    // curves; collapsing both paths to a single Line implementation keeps
    // intersection / projection / parameter-mapping logic uniform.
    use crate::primitives::curve::Line;
    let point = Vector3::from(vertex_data.position);
    let point_curve = Line::new(point, point);
    let curve_id = model.curves.add(Box::new(point_curve));

    let edge = Edge::new(
        0, // ID will be assigned by store
        vertex,
        vertex,
        curve_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Check whether the revolution axis passes through the face.
///
/// Three conditions detect intersection:
///
///  1. **Vertex on axis** — any boundary vertex within `tolerance` radial
///     distance of the axis. Cheap, catches sketches drawn touching the
///     pivot.
///  2. **Edge crossing axis** — sampled radial distance falls below
///     `tolerance` along an edge, *or* the radial offset vector flips
///     sense between samples (sign change in a fixed orthogonal frame).
///     Catches edges that pass through the axis without endpointing on it.
///  3. **Axis pierces face interior** — for a planar face the revolution
///     axis line is intersected with the face plane; the resulting point
///     is then tested against the face's outer loop using a 2D
///     point-in-polygon parity test on the axis-projected polygon. Catches
///     the "axis goes straight through the middle of a flat face" case.
///
/// Non-planar surfaces fall back to (1) and (2) only — sufficient in
/// practice because revolution profiles are typically sketched on
/// planar sketch planes.
fn face_intersects_axis(
    model: &BRepModel,
    face: &Face,
    axis_origin: Point3,
    axis_direction: Vector3,
) -> OperationResult<bool> {
    use crate::primitives::surface::Plane;

    let tolerance = 1e-6;
    let axis_dir = axis_direction
        .normalize()
        .unwrap_or(axis_direction);

    let loop_data = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

    // Helper: radial offset of a point from the infinite axis line.
    let radial_offset = |p: Point3| -> Vector3 {
        let to_p = Vector3::new(p.x, p.y, p.z) - Vector3::new(axis_origin.x, axis_origin.y, axis_origin.z);
        to_p - axis_dir * to_p.dot(&axis_dir)
    };

    // (1) + (2): walk the boundary loop, checking endpoints and edge interior.
    let mut radial_samples: Vec<Vector3> = Vec::new();
    for &edge_id in &loop_data.edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Endpoint check — fast path catches sketches touching the axis.
        for &vertex_id in &[edge.start_vertex, edge.end_vertex] {
            let vertex = model
                .vertices
                .get(vertex_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
            let point = Point3::new(
                vertex.position[0],
                vertex.position[1],
                vertex.position[2],
            );
            let r = radial_offset(point);
            if r.magnitude() < tolerance {
                return Ok(true);
            }
            radial_samples.push(r);
        }

        // Edge-interior check: sample the curve and look for sub-tolerance
        // radial magnitude or sign-flip in the radial offset direction.
        if let Some(curve) = model.curves.get(edge.curve_id) {
            let pr = curve.parameter_range();
            let span = pr.end - pr.start;
            if span.abs() > 1e-12 {
                const N: usize = 8;
                let mut prev: Option<Vector3> = None;
                for i in 0..=N {
                    let t = pr.start + span * (i as f64 / N as f64);
                    if let Ok(p) = curve.point_at(t) {
                        let r = radial_offset(p);
                        if r.magnitude() < tolerance {
                            return Ok(true);
                        }
                        if let Some(prev_r) = prev {
                            // If the offset vectors point in opposite
                            // half-spaces, the edge crossed the axis line.
                            if prev_r.dot(&r) < 0.0 {
                                return Ok(true);
                            }
                        }
                        prev = Some(r);
                    }
                }
            }
        }
    }

    // (3) Planar-face interior pierce test.
    if let Some(surface) = model.surfaces.get(face.surface_id) {
        if let Some(plane) = surface.as_any().downcast_ref::<Plane>() {
            let n = plane.normal;
            let denom = n.dot(&axis_dir);
            if denom.abs() > 1e-12 {
                // Axis is not parallel to plane → unique intersection point.
                let plane_origin_v =
                    Vector3::new(plane.origin.x, plane.origin.y, plane.origin.z);
                let axis_origin_v =
                    Vector3::new(axis_origin.x, axis_origin.y, axis_origin.z);
                let t = n.dot(&(plane_origin_v - axis_origin_v)) / denom;
                let pierce = Point3::new(
                    axis_origin.x + axis_dir.x * t,
                    axis_origin.y + axis_dir.y * t,
                    axis_origin.z + axis_dir.z * t,
                );

                // Build a 2D frame on the plane to run point-in-polygon.
                let u_dir = if n.x.abs() < 0.9 {
                    n.cross(&Vector3::new(1.0, 0.0, 0.0))
                } else {
                    n.cross(&Vector3::new(0.0, 1.0, 0.0))
                }
                .normalize()
                .unwrap_or(Vector3::X);
                let v_dir = n.cross(&u_dir).normalize().unwrap_or(Vector3::Y);

                let project_2d = |p: Point3| -> (f64, f64) {
                    let d = Vector3::new(p.x, p.y, p.z) - plane_origin_v;
                    (d.dot(&u_dir), d.dot(&v_dir))
                };

                // Collect ordered boundary vertices.
                let mut polygon: Vec<(f64, f64)> = Vec::new();
                for &edge_id in &loop_data.edges {
                    if let Some(edge) = model.edges.get(edge_id) {
                        if let Some(vertex) = model.vertices.get(edge.start_vertex) {
                            let p = Point3::new(
                                vertex.position[0],
                                vertex.position[1],
                                vertex.position[2],
                            );
                            polygon.push(project_2d(p));
                        }
                    }
                }

                if let Some(&last) = polygon.last() {
                    if polygon.len() >= 3 {
                        let (px, py) = project_2d(pierce);
                        let mut inside = false;
                        let mut prev = last;
                        for &curr in &polygon {
                            let (xi, yi) = curr;
                            let (xj, yj) = prev;
                            let crosses = (yi > py) != (yj > py)
                                && px < (xj - xi) * (py - yi) / (yj - yi) + xi;
                            if crosses {
                                inside = !inside;
                            }
                            prev = curr;
                        }
                        if inside {
                            return Ok(true);
                        }
                    }
                }
            }
        }
    }

    Ok(false)
}

/// Validate inputs for revolution
fn validate_revolve_inputs(
    model: &BRepModel,
    face_id: FaceId,
    options: &RevolveOptions,
) -> OperationResult<()> {
    // Check face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    // Check angle is valid
    if options.angle <= 0.0 || options.angle > std::f64::consts::TAU * 2.0 {
        return Err(OperationError::InvalidGeometry(
            "Invalid revolution angle".to_string(),
        ));
    }

    // Check axis direction is valid
    if options.axis_direction.magnitude() < options.common.tolerance.distance() {
        return Err(OperationError::InvalidGeometry(
            "Invalid axis direction".to_string(),
        ));
    }

    // Check segments is reasonable
    if options.segments < 3 {
        return Err(OperationError::InvalidGeometry(
            "Too few segments for revolution".to_string(),
        ));
    }

    Ok(())
}

/// Validate the revolved solid
fn validate_revolved_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    // Would perform full B-Rep validation
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep("Solid not found".to_string()));
    }

    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_revolution_validation() {
//         // Test validation of revolution parameters
//     }
// }
