//! Extrusion Operations for B-Rep Models
//!
//! Implements face and profile extrusion with draft angles, twist, and taper.
//! All operations maintain exact analytical geometry.
//!
//! # References
//! - Stroud, I. (2006). Boundary Representation Modelling Techniques. Springer.
//! - Mäntylä, M. (1988). An Introduction to Solid Modeling. Computer Science Press.

use super::deep_clone::deep_clone_faces;
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
use tracing::debug;

/// Find the solid that contains the given face
fn find_parent_solid(model: &BRepModel, face_id: FaceId) -> Option<SolidId> {
    // Iterate through all solids by index
    for solid_id in 0..model.solids.len() {
        let solid_id = solid_id as SolidId;
        if let Some(solid) = model.solids.get(solid_id) {
            // Check outer shell
            if let Some(shell) = model.shells.get(solid.outer_shell) {
                if shell.faces.contains(&face_id) {
                    return Some(solid_id);
                }
            }
            // Also check inner shells (for solids with holes)
            for &inner_shell_id in &solid.inner_shells {
                if let Some(shell) = model.shells.get(inner_shell_id) {
                    if shell.faces.contains(&face_id) {
                        return Some(solid_id);
                    }
                }
            }
        }
    }
    None
}

/// Options for extrusion operations
#[derive(Debug, Clone)]
pub struct ExtrudeOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Extrusion direction (will be normalized)
    pub direction: Vector3,

    /// Extrusion distance (positive or negative)
    pub distance: f64,

    /// Whether extrusion is symmetric (extends in both directions)
    pub symmetric: bool,

    /// Draft angle in radians (0 = straight, positive = outward taper)
    pub draft_angle: f64,

    /// Twist angle in radians over the full distance
    pub twist_angle: f64,

    /// Whether to cap the ends (false for thin extrusion)
    pub cap_ends: bool,

    /// Scale factor at the end (1.0 = no scaling)
    pub end_scale: f64,
}

impl Default for ExtrudeOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            direction: Vector3::Z,
            distance: 1.0,
            symmetric: false,
            draft_angle: 0.0,
            twist_angle: 0.0,
            cap_ends: true,
            end_scale: 1.0,
        }
    }
}

/// Extrude a face along a direction to create a solid or sheet
pub fn extrude_face(
    model: &mut BRepModel,
    face_id: FaceId,
    options: ExtrudeOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_extrude_inputs(model, face_id, &options)?;

    // Find the parent solid that contains this face
    let parent_solid_id = find_parent_solid(model, face_id).ok_or_else(|| {
        OperationError::InvalidGeometry("Face is not part of any solid".to_string())
    })?;

    // Normalize direction
    let direction = options.direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Direction normalization failed: {:?}", e))
    })?;

    // Get the face to extrude
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Route to complex extrusion when draft, twist, or taper are active
    let has_complex_options = options.draft_angle.abs() > 1e-10
        || options.twist_angle.abs() > 1e-10
        || (options.end_scale - 1.0).abs() > 1e-10;

    let unified_solid_id = if has_complex_options {
        create_complex_unified_extrusion(model, parent_solid_id, &face, face_id, &options)?
    } else {
        create_unified_extrusion(
            model,
            parent_solid_id,
            &face,
            face_id,
            direction,
            options.distance,
            options.cap_ends,
        )?
    };

    // Validate result if requested
    if options.common.validate_result {
        validate_extruded_solid(model, unified_solid_id)?;
    }

    // Record for attached recorders. `direction` above was moved into
    // `create_*_unified_extrusion`, so re-read from `options` (the option
    // struct is still borrowed and un-normalized — sufficient for a record).
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("extrude_face")
            .with_parameters(serde_json::json!({
                "face_id": face_id,
                "distance": options.distance,
                "direction": [options.direction.x, options.direction.y, options.direction.z],
                "cap_ends": options.cap_ends,
                "draft_angle": options.draft_angle,
                "twist_angle": options.twist_angle,
                "end_scale": options.end_scale,
            }))
            .with_inputs(vec![face_id as u64])
            .with_outputs(vec![unified_solid_id as u64]),
    );

    Ok(unified_solid_id)
}

/// Create a unified extrusion that combines the original solid with the extruded volume
fn create_unified_extrusion(
    model: &mut BRepModel,
    parent_solid_id: SolidId,
    base_face: &Face,
    base_face_id: FaceId,
    direction: Vector3,
    distance: f64,
    cap_ends: bool,
) -> OperationResult<SolidId> {
    // Get the parent solid and shell
    let parent_solid = model
        .solids
        .get(parent_solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Parent solid not found".to_string()))?
        .clone();

    let parent_shell = model
        .shells
        .get(parent_solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Parent shell not found".to_string()))?
        .clone();

    // Create new shell faces for the unified solid
    let mut unified_faces = Vec::new();

    // 1. Deep clone all faces from parent EXCEPT the base face being extruded
    let cloned_faces = deep_clone_faces(model, &parent_shell.faces, &[base_face_id])?;
    unified_faces.extend(cloned_faces);

    // 2. Create side faces by extruding each edge of the base face
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    for (i, &edge_id) in base_loop.edges.iter().enumerate() {
        let edge_forward = base_loop.orientations[i];
        let side_face =
            create_extruded_edge_face(model, edge_id, edge_forward, direction, distance)?;
        unified_faces.push(side_face);
    }

    // 3. Create the top face (translated base face)
    if cap_ends {
        let top_face = create_translated_face(model, base_face, direction * distance)?;
        unified_faces.push(top_face);
    }

    // 4. Create new unified shell
    let mut unified_shell = Shell::new(0, ShellType::Closed);
    for &face_id in &unified_faces {
        unified_shell.add_face(face_id);
    }
    let unified_shell_id = model.shells.add(unified_shell);

    // Log the unified shell information
    debug!(
        shell_id = unified_shell_id,
        face_count = unified_faces.len(),
        "Created unified shell"
    );
    for (i, &face_id) in unified_faces.iter().enumerate() {
        if let Some(face) = model.faces.get(face_id) {
            if model.surfaces.get(face.surface_id).is_some() {
                debug!(
                    face_idx = i,
                    face_id,
                    surface_id = face.surface_id,
                    "  Face"
                );
            }
        }
    }

    // 5. Update the parent solid with the new unified shell
    // This maintains the same solid ID instead of creating a new one
    if let Some(parent_solid) = model.solids.get_mut(parent_solid_id) {
        parent_solid.outer_shell = unified_shell_id;
        debug!(
            solid_id = parent_solid_id,
            shell_id = unified_shell_id,
            "Updated parent solid with new shell"
        );
        Ok(parent_solid_id)
    } else {
        // Fallback: create new solid if parent update fails
        let unified_solid = Solid::new(0, unified_shell_id);
        let unified_solid_id = model.solids.add(unified_solid);
        debug!(
            solid_id = unified_solid_id,
            shell_id = unified_shell_id,
            "Created new solid with shell"
        );
        Ok(unified_solid_id)
    }
}

/// Create a unified extrusion with draft angle, twist, and/or end scale.
///
/// Replaces the parent solid's shell with a new one that removes the extruded
/// base face and adds side + top faces computed via the complex transformation
/// pipeline (draft radial offset, twist rotation, end scale).
fn create_complex_unified_extrusion(
    model: &mut BRepModel,
    parent_solid_id: SolidId,
    base_face: &Face,
    base_face_id: FaceId,
    options: &ExtrudeOptions,
) -> OperationResult<SolidId> {
    let direction = options.direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Direction normalization failed: {:?}", e))
    })?;

    let parent_solid = model
        .solids
        .get(parent_solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Parent solid not found".to_string()))?
        .clone();

    let parent_shell = model
        .shells
        .get(parent_solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Parent shell not found".to_string()))?
        .clone();

    // Clone all faces from parent EXCEPT the base face being extruded
    let mut unified_faces = deep_clone_faces(model, &parent_shell.faces, &[base_face_id])?;

    // Get base loop vertices
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut prev_vertices: Vec<VertexId> = Vec::new();
    for &edge_id in &base_loop.edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        prev_vertices.push(edge.start_vertex);
    }

    // Compute face centroid for radial draft offset
    let face_centroid = {
        let mut cx = 0.0;
        let mut cy = 0.0;
        let mut cz = 0.0;
        let mut count = 0usize;
        for &vid in &prev_vertices {
            if let Some(v) = model.vertices.get(vid) {
                cx += v.position[0];
                cy += v.position[1];
                cz += v.position[2];
                count += 1;
            }
        }
        if count == 0 {
            return Err(OperationError::InvalidGeometry(
                "Face has no valid vertices for centroid computation".to_string(),
            ));
        }
        let n = count as f64;
        Point3::new(cx / n, cy / n, cz / n)
    };

    // Steps: more for twist (smooth approximation), fewer otherwise
    let num_steps = if options.twist_angle.abs() > 1e-10 {
        10
    } else {
        1
    };
    let step_distance = options.distance / num_steps as f64;
    let step_twist = options.twist_angle / num_steps as f64;
    let step_scale = (options.end_scale - 1.0) / num_steps as f64;
    let step_draft_tan = options.draft_angle.tan();

    for step in 1..=num_steps {
        let current_distance = step_distance * step as f64;
        let current_twist = step_twist * step as f64;
        let current_scale = 1.0 + step_scale * step as f64;
        let current_draft_offset = current_distance * step_draft_tan;

        let translation = Matrix4::from_translation(&(direction * current_distance));
        let rotation = if options.twist_angle.abs() > 1e-10 {
            Matrix4::from_axis_angle(&direction, current_twist).map_err(|e| {
                OperationError::NumericalError(format!("Rotation matrix failed: {:?}", e))
            })?
        } else {
            Matrix4::IDENTITY
        };
        let scaling = Matrix4::uniform_scale(current_scale);
        let transform = translation * rotation * scaling;

        let mut current_vertices = Vec::new();
        for &vertex_id in &prev_vertices {
            let vertex = model
                .vertices
                .get(vertex_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
            let mut pos = Point3::from(vertex.position);

            // Draft: offset radially from centroid in plane perpendicular to direction
            if options.draft_angle.abs() > 1e-10 {
                let to_vertex = pos - face_centroid;
                let radial = to_vertex - direction * to_vertex.dot(&direction);
                let radial_dir = radial.normalize().unwrap_or(Vector3::X);
                pos = pos + radial_dir * current_draft_offset;
            }

            let transformed_pos = transform.transform_point(&pos);
            let new_vertex =
                model
                    .vertices
                    .add(transformed_pos.x, transformed_pos.y, transformed_pos.z);
            current_vertices.push(new_vertex);
        }

        // Side faces between prev and current rings
        for i in 0..prev_vertices.len() {
            let next_i = (i + 1) % prev_vertices.len();
            let face_id = create_quad_face(
                model,
                prev_vertices[i],
                prev_vertices[next_i],
                current_vertices[next_i],
                current_vertices[i],
            )?;
            unified_faces.push(face_id);
        }

        prev_vertices = current_vertices;
    }

    // Cap the top
    if options.cap_ends {
        let top_face = create_face_from_vertices(model, &prev_vertices)?;
        unified_faces.push(top_face);
    }

    // Build unified shell and update parent
    let mut unified_shell = Shell::new(0, ShellType::Closed);
    for &fid in &unified_faces {
        unified_shell.add_face(fid);
    }
    let unified_shell_id = model.shells.add(unified_shell);

    if let Some(parent) = model.solids.get_mut(parent_solid_id) {
        parent.outer_shell = unified_shell_id;
        Ok(parent_solid_id)
    } else {
        let solid = Solid::new(0, unified_shell_id);
        let solid_id = model.solids.add(solid);
        Ok(solid_id)
    }
}

/// Extrude a wire/profile to create a solid
pub fn extrude_profile(
    model: &mut BRepModel,
    profile_edges: Vec<EdgeId>,
    options: ExtrudeOptions,
) -> OperationResult<SolidId> {
    // First create a face from the profile
    let face_id = create_face_from_profile(model, profile_edges)?;

    // Then extrude the face
    extrude_face(model, face_id, options)
}

/// Create a simple linear extrusion (most common case)
fn create_linear_extrusion(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    direction: Vector3,
    distance: f64,
    cap_ends: bool,
) -> OperationResult<SolidId> {
    let mut shell_faces = Vec::new();

    // Add base face (reversed if needed for correct orientation)
    if distance > 0.0 {
        shell_faces.push(base_face_id);
    } else {
        // Need to reverse the face
        let reversed_face = create_reversed_face(model, base_face)?;
        shell_faces.push(reversed_face);
    }

    // Create side faces by extruding each edge
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    for (i, &edge_id) in base_loop.edges.iter().enumerate() {
        let edge_forward = base_loop.orientations[i];
        let side_face =
            create_extruded_edge_face(model, edge_id, edge_forward, direction, distance)?;
        shell_faces.push(side_face);
    }

    // Create top face if capping
    if cap_ends {
        let top_face = create_translated_face(model, base_face, direction * distance)?;
        shell_faces.push(top_face);
    }

    // Create shell and solid
    let shell = Shell::new(0, ShellType::Closed); // Will be assigned by store
    let mut shell = shell;
    for face_id in &shell_faces {
        shell.add_face(*face_id);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id); // Will be assigned by store
    let solid_id = model.solids.add(solid);

    Ok(solid_id)
}

/// Create a complex extrusion with draft, twist, or scale
fn create_complex_extrusion(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    options: &ExtrudeOptions,
) -> OperationResult<SolidId> {
    let direction = options.direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Direction normalization failed: {:?}", e))
    })?;

    let mut shell_faces = Vec::new();

    // Add base face
    shell_faces.push(base_face_id);

    // Get base loop
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    // Create transformation matrices for each step along extrusion
    let num_steps = if options.twist_angle.abs() > 1e-10 {
        10
    } else {
        1
    };
    let step_distance = options.distance / num_steps as f64;
    let step_twist = options.twist_angle / num_steps as f64;
    let step_scale = (options.end_scale - 1.0) / num_steps as f64;
    let step_draft_tan = options.draft_angle.tan();

    // Create intermediate cross-sections for complex extrusion
    let mut prev_vertices = Vec::new();

    // Get vertices from base loop and store them
    for &edge_id in &base_loop.edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        prev_vertices.push(edge.start_vertex);
    }

    // Compute face centroid from base vertices for draft angle calculation.
    // The centroid is projected onto the plane perpendicular to the extrusion direction
    // so that radial draft offsets are applied correctly in 3D.
    let face_centroid = {
        let mut cx = 0.0;
        let mut cy = 0.0;
        let mut cz = 0.0;
        let mut count = 0usize;
        for &vid in &prev_vertices {
            if let Some(v) = model.vertices.get(vid) {
                cx += v.position[0];
                cy += v.position[1];
                cz += v.position[2];
                count += 1;
            }
        }
        if count == 0 {
            return Err(OperationError::InvalidGeometry(
                "Face has no valid vertices for centroid computation".to_string(),
            ));
        }
        let n = count as f64;
        Point3::new(cx / n, cy / n, cz / n)
    };

    // Create vertices and faces for each step
    for step in 1..=num_steps {
        let current_distance = step_distance * step as f64;
        let current_twist = step_twist * step as f64;
        let current_scale = 1.0 + step_scale * step as f64;
        let current_draft_offset = current_distance * step_draft_tan;

        // Create transformation matrix for this step
        let translation = Matrix4::from_translation(&(direction * current_distance));
        let rotation = if options.twist_angle.abs() > 1e-10 {
            Matrix4::from_axis_angle(&direction, current_twist).map_err(|e| {
                OperationError::NumericalError(format!("Rotation matrix failed: {:?}", e))
            })?
        } else {
            Matrix4::IDENTITY
        };
        let scaling = Matrix4::uniform_scale(current_scale);

        // Combine transformations
        let transform = translation * rotation * scaling;

        // Create new vertices for this step
        let mut current_vertices = Vec::new();
        for &vertex_id in &prev_vertices {
            let vertex = model
                .vertices
                .get(vertex_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

            let mut pos = Point3::from(vertex.position);

            // Apply draft angle: offset each vertex radially away from the face centroid,
            // in the plane perpendicular to the extrusion direction.
            if options.draft_angle.abs() > 1e-10 {
                let to_vertex = pos - face_centroid;
                // Project onto plane perpendicular to extrusion direction
                let radial = to_vertex - direction * to_vertex.dot(&direction);
                let radial_dir = radial.normalize().unwrap_or(Vector3::X);
                pos = pos + radial_dir * current_draft_offset;
            }

            // Apply transformation
            let transformed_pos = transform.transform_point(&pos);
            let new_vertex =
                model
                    .vertices
                    .add(transformed_pos.x, transformed_pos.y, transformed_pos.z);
            current_vertices.push(new_vertex);
        }

        // Create side faces between previous and current vertices
        for i in 0..prev_vertices.len() {
            let next_i = (i + 1) % prev_vertices.len();

            let v1 = prev_vertices[i];
            let v2 = prev_vertices[next_i];
            let v3 = current_vertices[next_i];
            let v4 = current_vertices[i];

            // Create quadrilateral face
            let face_id = create_quad_face(model, v1, v2, v3, v4)?;
            shell_faces.push(face_id);
        }

        prev_vertices = current_vertices;
    }

    // Create top face if capping
    if options.cap_ends {
        let top_face = create_face_from_vertices(model, &prev_vertices)?;
        shell_faces.push(top_face);
    }

    // Create shell and solid
    let shell = Shell::new(0, ShellType::Closed);
    let mut shell = shell;
    for face_id in &shell_faces {
        shell.add_face(*face_id);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id);
    let solid_id = model.solids.add(solid);

    Ok(solid_id)
}

/// Create a face by extruding an edge
fn create_extruded_edge_face(
    model: &mut BRepModel,
    edge_id: EdgeId,
    edge_forward: bool,
    direction: Vector3,
    distance: f64,
) -> OperationResult<FaceId> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Get edge endpoints
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?
        .clone();
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?
        .clone();

    // Create translated vertices
    let start_pos_top = Point3::from(start_vertex.position) + direction * distance;
    let end_pos_top = Point3::from(end_vertex.position) + direction * distance;

    let top_start = model
        .vertices
        .add(start_pos_top.x, start_pos_top.y, start_pos_top.z);
    let top_end = model
        .vertices
        .add(end_pos_top.x, end_pos_top.y, end_pos_top.z);

    // Create edges for the face
    let mut face_edges = Vec::new();

    // Bottom edge (original)
    face_edges.push((edge_id, edge_forward));

    // Right edge (end vertex to top)
    let right_edge = create_straight_edge(
        model,
        if edge_forward {
            edge.end_vertex
        } else {
            edge.start_vertex
        },
        if edge_forward { top_end } else { top_start },
    )?;
    face_edges.push((right_edge, true));

    // Top edge (reversed)
    let top_edge = create_edge_translation(model, &edge, direction * distance)?;
    face_edges.push((top_edge, !edge_forward));

    // Left edge (start vertex to top, reversed)
    let left_edge = create_straight_edge(
        model,
        if edge_forward {
            edge.start_vertex
        } else {
            edge.end_vertex
        },
        if edge_forward { top_start } else { top_end },
    )?;
    face_edges.push((left_edge, false));

    // Create loop
    let mut face_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer); // Will be assigned by store
    for (edge_id, forward) in face_edges {
        face_loop.add_edge(edge_id, forward);
    }
    let loop_id = model.loops.add(face_loop);

    // Create ruled surface between bottom and top edges
    let surface = create_ruled_surface(model, edge_id, top_edge)?;
    let surface_id = model.surfaces.add(surface);

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

/// Create a straight line edge between two vertices
fn create_straight_edge(
    model: &mut BRepModel,
    start_vertex: VertexId,
    end_vertex: VertexId,
) -> OperationResult<EdgeId> {
    use crate::primitives::curve::Line;

    let start = model
        .vertices
        .get(start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end = model
        .vertices
        .get(end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let line = Line::new(Point3::from(start.position), Point3::from(end.position));
    let curve_id = model.curves.add(Box::new(line));

    let edge = Edge::new_auto_range(
        0, // Will be assigned by store
        start_vertex,
        end_vertex,
        curve_id,
        EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Create a translated copy of an edge
fn create_edge_translation(
    model: &mut BRepModel,
    edge: &Edge,
    translation: Vector3,
) -> OperationResult<EdgeId> {
    // Get original curve
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    // Create translated curve
    let translated_curve = curve.transform(&Matrix4::from_translation(&translation));
    let new_curve_id = model.curves.add(translated_curve);

    // Get translated vertices
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let new_start_pos = Point3::from(start_vertex.position) + translation;
    let new_end_pos = Point3::from(end_vertex.position) + translation;

    let new_start = model
        .vertices
        .add(new_start_pos.x, new_start_pos.y, new_start_pos.z);
    let new_end = model
        .vertices
        .add(new_end_pos.x, new_end_pos.y, new_end_pos.z);

    // Create new edge
    let new_edge = Edge::new(
        0, // Will be assigned by store
        new_start,
        new_end,
        new_curve_id,
        edge.orientation,
        edge.param_range,
    );
    let edge_id = model.edges.add(new_edge);

    Ok(edge_id)
}

/// Create a ruled surface between two edges
fn create_ruled_surface(
    model: &mut BRepModel,
    bottom_edge: EdgeId,
    top_edge: EdgeId,
) -> OperationResult<Box<dyn Surface>> {
    // Get the two curves
    let bottom_edge_data = model
        .edges
        .get(bottom_edge)
        .ok_or_else(|| OperationError::InvalidGeometry("Bottom edge not found".to_string()))?;
    let top_edge_data = model
        .edges
        .get(top_edge)
        .ok_or_else(|| OperationError::InvalidGeometry("Top edge not found".to_string()))?;

    let bottom_curve = model
        .curves
        .get(bottom_edge_data.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Bottom curve not found".to_string()))?;
    let top_curve = model
        .curves
        .get(top_edge_data.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Top curve not found".to_string()))?;

    // Create a ruled surface by interpolating between the two curves
    // For ruled surfaces: S(u,v) = (1-v) * C1(u) + v * C2(u)
    // where C1 and C2 are the two boundary curves

    // Sample points from both curves to create a NURBS surface
    let num_samples = 10;
    let mut control_points = Vec::new();
    let mut weights = Vec::new();

    for i in 0..=num_samples {
        let u = i as f64 / num_samples as f64;

        // Get points on both curves
        let bottom_point = bottom_curve.point_at(u).map_err(|e| {
            OperationError::NumericalError(format!("Bottom curve evaluation failed: {:?}", e))
        })?;
        let top_point = top_curve.point_at(u).map_err(|e| {
            OperationError::NumericalError(format!("Top curve evaluation failed: {:?}", e))
        })?;

        // Create two rows of control points
        control_points.push(bottom_point);
        control_points.push(top_point);
        weights.push(1.0);
        weights.push(1.0);
    }

    // For now, create a planar approximation between the endpoints
    // A complete implementation would create a proper NURBS ruled surface
    let bottom_start = bottom_curve.point_at(0.0).map_err(|e| {
        OperationError::NumericalError(format!("Bottom curve start evaluation failed: {:?}", e))
    })?;
    let bottom_end = bottom_curve.point_at(1.0).map_err(|e| {
        OperationError::NumericalError(format!("Bottom curve end evaluation failed: {:?}", e))
    })?;
    let top_start = top_curve.point_at(0.0).map_err(|e| {
        OperationError::NumericalError(format!("Top curve start evaluation failed: {:?}", e))
    })?;

    // Create a plane from three points (bottom_start, bottom_end, top_start)
    let v1 = Vector3::from(bottom_end - bottom_start);
    let v2 = Vector3::from(top_start - bottom_start);
    let normal = v1.cross(&v2).normalize().map_err(|e| {
        OperationError::NumericalError(format!("Normal calculation failed: {:?}", e))
    })?;

    use crate::primitives::surface::Plane;
    let plane = Plane::from_point_normal(bottom_start, normal)
        .map_err(|e| OperationError::NumericalError(format!("Plane creation failed: {:?}", e)))?;
    Ok(Box::new(plane))
}

/// Create a face from a closed wire profile
pub fn create_face_from_profile(
    model: &mut BRepModel,
    profile_edges: Vec<EdgeId>,
) -> OperationResult<FaceId> {
    // Validate that edges form a closed loop
    validate_closed_profile(model, &profile_edges)?;

    // Create loop from edges
    let mut profile_loop = Loop::new(
        0, // Will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    for &edge_id in &profile_edges {
        profile_loop.add_edge(edge_id, true);
    }
    let loop_id = model.loops.add(profile_loop);

    // Create a planar surface (assuming profile is planar)
    let surface = create_planar_surface_from_edges(model, &profile_edges)?;
    let surface_id = model.surfaces.add(surface);

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

/// Create a reversed copy of a face
fn create_reversed_face(model: &mut BRepModel, face: &Face) -> OperationResult<FaceId> {
    let mut reversed_face = face.clone();
    reversed_face.id = 0; // Will be assigned by store
    reversed_face.orientation = match face.orientation {
        FaceOrientation::Forward => FaceOrientation::Backward,
        FaceOrientation::Backward => FaceOrientation::Forward,
    };

    let face_id = model.faces.add(reversed_face);
    Ok(face_id)
}

/// Create a translated copy of a face
fn create_translated_face(
    model: &mut BRepModel,
    face: &Face,
    translation: Vector3,
) -> OperationResult<FaceId> {
    // Transform the surface
    let original_surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
    let translated_surface = original_surface.transform(&Matrix4::from_translation(&translation));
    let new_surface_id = model.surfaces.add(translated_surface);

    // Transform the outer loop
    let translated_outer_loop = translate_loop(model, face.outer_loop, translation)?;
    let new_outer_loop_id = model.loops.add(translated_outer_loop);

    // Transform inner loops if any
    let mut new_inner_loops = Vec::new();
    for &inner_loop_id in &face.inner_loops {
        let translated_inner_loop = translate_loop(model, inner_loop_id, translation)?;
        let new_inner_loop_id = model.loops.add(translated_inner_loop);
        new_inner_loops.push(new_inner_loop_id);
    }

    // Create new face with reversed orientation for top face
    let mut new_face = Face::new(
        0, // Will be assigned by store
        new_surface_id,
        new_outer_loop_id,
        match face.orientation {
            FaceOrientation::Forward => FaceOrientation::Backward,
            FaceOrientation::Backward => FaceOrientation::Forward,
        },
    );

    // Add inner loops
    for inner_loop_id in new_inner_loops {
        new_face.add_inner_loop(inner_loop_id);
    }

    let face_id = model.faces.add(new_face);
    Ok(face_id)
}

/// Create a planar surface from a set of edges
fn create_planar_surface_from_edges(
    model: &mut BRepModel,
    edges: &[EdgeId],
) -> OperationResult<Box<dyn Surface>> {
    if edges.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Need at least 3 edges to define a plane".to_string(),
        ));
    }

    // Get points from the edges to compute the plane
    let mut points = Vec::new();

    for &edge_id in edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        let curve = model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

        // Sample points along the edge
        for i in 0..=2 {
            let t = i as f64 / 2.0;
            let point = curve.point_at(t).map_err(|e| {
                OperationError::NumericalError(format!("Curve evaluation failed: {:?}", e))
            })?;
            points.push(point);
        }
    }

    // Find the best-fit plane using least squares
    // For now, use first three non-collinear points
    let mut plane_points = Vec::new();
    for i in 0..points.len() {
        if plane_points.len() >= 3 {
            break;
        }

        let point = points[i];
        let mut is_collinear = false;

        if plane_points.len() >= 2 {
            let v1 = Vector3::from(plane_points[1] - plane_points[0]);
            let v2 = Vector3::from(point - plane_points[0]);
            if v1.cross(&v2).magnitude() < 1e-10 {
                is_collinear = true;
            }
        }

        if !is_collinear {
            plane_points.push(point);
        }
    }

    if plane_points.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Cannot find three non-collinear points".to_string(),
        ));
    }

    // Create plane from three points
    let origin = plane_points[0];
    let v1 = Vector3::from(plane_points[1] - plane_points[0]);
    let v2 = Vector3::from(plane_points[2] - plane_points[0]);
    let normal = v1.cross(&v2).normalize().map_err(|e| {
        OperationError::NumericalError(format!("Normal calculation failed: {:?}", e))
    })?;

    use crate::primitives::surface::Plane;
    let plane = Plane::from_point_normal(origin, normal)
        .map_err(|e| OperationError::NumericalError(format!("Plane creation failed: {:?}", e)))?;
    Ok(Box::new(plane))
}

/// Validate inputs for extrusion
fn validate_extrude_inputs(
    model: &BRepModel,
    face_id: FaceId,
    options: &ExtrudeOptions,
) -> OperationResult<()> {
    // Check face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Face not found".to_string(),
        ));
    }

    // Check distance is non-zero
    if options.distance.abs() < options.common.tolerance.distance() {
        return Err(OperationError::InvalidGeometry(
            "Extrusion distance too small".to_string(),
        ));
    }

    // Check direction is valid
    if options.direction.magnitude() < options.common.tolerance.distance() {
        return Err(OperationError::InvalidGeometry(
            "Invalid extrusion direction".to_string(),
        ));
    }

    // Check draft angle is reasonable — must be strictly less than 90 degrees
    if options.draft_angle.abs() >= std::f64::consts::FRAC_PI_2 {
        return Err(OperationError::InvalidGeometry(format!(
            "Draft angle {:.4} radians exceeds maximum (must be less than 90 degrees)",
            options.draft_angle
        )));
    }

    Ok(())
}

/// Validate that edges form a closed profile
fn validate_closed_profile(model: &BRepModel, edges: &[EdgeId]) -> OperationResult<()> {
    if edges.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "No edges in profile".to_string(),
        ));
    }

    // Check that edges connect end-to-end
    // Simplified check - full implementation would handle edge orientations
    for i in 0..edges.len() {
        let current = model
            .edges
            .get(edges[i])
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        let next = model
            .edges
            .get(edges[(i + 1) % edges.len()])
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Check connectivity (simplified)
        if current.end_vertex != next.start_vertex && current.end_vertex != next.end_vertex {
            return Err(OperationError::OpenProfile);
        }
    }

    Ok(())
}

/// Validate the extruded solid
fn validate_extruded_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    // Would perform full B-Rep validation
    // For now, just check it exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep("Solid not found".to_string()));
    }

    Ok(())
}

/// Translate a loop by a given vector
fn translate_loop(
    model: &mut BRepModel,
    loop_id: u32,
    translation: Vector3,
) -> OperationResult<Loop> {
    let original_loop = model
        .loops
        .get(loop_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut new_loop = Loop::new(0, original_loop.loop_type);

    // Translate each edge in the loop
    for (i, &edge_id) in original_loop.edges.iter().enumerate() {
        let forward = original_loop.orientations[i];
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
            .clone(); // Clone to avoid borrowing issues

        let translated_edge = create_edge_translation(model, &edge, translation)?;
        new_loop.add_edge(translated_edge, forward);
    }

    Ok(new_loop)
}

/// Create a quadrilateral face from four vertices
fn create_quad_face(
    model: &mut BRepModel,
    v1: VertexId,
    v2: VertexId,
    v3: VertexId,
    v4: VertexId,
) -> OperationResult<FaceId> {
    debug!(v1, v2, v3, v4, "create_quad_face called");

    // Check if vertices are distinct
    if v1 == v2 || v2 == v3 || v3 == v4 || v4 == v1 || v1 == v3 || v2 == v4 {
        return Err(OperationError::InvalidGeometry(
            "Degenerate quad with duplicate vertices".to_string(),
        ));
    }

    // Log vertex positions
    if let (Some(vp1), Some(vp2), Some(vp3), Some(vp4)) = (
        model.vertices.get(v1),
        model.vertices.get(v2),
        model.vertices.get(v3),
        model.vertices.get(v4),
    ) {
        debug!(?vp1.position, ?vp2.position, ?vp3.position, ?vp4.position, "quad vertex positions");
    }

    // Create edges for the quad
    let edge1 = create_straight_edge(model, v1, v2)?;
    let edge2 = create_straight_edge(model, v2, v3)?;
    let edge3 = create_straight_edge(model, v3, v4)?;
    let edge4 = create_straight_edge(model, v4, v1)?;

    // Create loop
    let mut face_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
    face_loop.add_edge(edge1, true);
    face_loop.add_edge(edge2, true);
    face_loop.add_edge(edge3, true);
    face_loop.add_edge(edge4, true);
    let loop_id = model.loops.add(face_loop);

    // Create planar surface from the four vertices
    let vertices = [v1, v2, v3, v4];
    let surface = create_planar_surface_from_vertices(model, &vertices)?;
    let surface_id = model.surfaces.add(surface);

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

/// Create a face from a list of vertices
fn create_face_from_vertices(
    model: &mut BRepModel,
    vertices: &[VertexId],
) -> OperationResult<FaceId> {
    if vertices.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Need at least 3 vertices for a face".to_string(),
        ));
    }

    // Create edges connecting the vertices
    let mut edges = Vec::new();
    for i in 0..vertices.len() {
        let next_i = (i + 1) % vertices.len();
        let edge = create_straight_edge(model, vertices[i], vertices[next_i])?;
        edges.push(edge);
    }

    // Create loop
    let mut face_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
    for edge_id in edges {
        face_loop.add_edge(edge_id, true);
    }
    let loop_id = model.loops.add(face_loop);

    // Create planar surface
    let surface = create_planar_surface_from_vertices(model, vertices)?;
    let surface_id = model.surfaces.add(surface);

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

/// Create a planar surface from a set of vertices
fn create_planar_surface_from_vertices(
    model: &mut BRepModel,
    vertices: &[VertexId],
) -> OperationResult<Box<dyn Surface>> {
    if vertices.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Need at least 3 vertices to define a plane".to_string(),
        ));
    }

    // Get the first three non-collinear vertices
    let mut points = Vec::new();

    debug!(
        vertex_count = vertices.len(),
        "create_planar_surface_from_vertices"
    );

    for &vertex_id in vertices {
        let vertex = model
            .vertices
            .get(vertex_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
        let point = Point3::from(vertex.position);

        debug!(vertex_id, ?point, "vertex position");
        points.push(point);

        if points.len() >= 3 {
            // Check if these three points are non-collinear
            let v1 = Vector3::from(points[1] - points[0]);
            let v2 = Vector3::from(points[2] - points[0]);

            debug!(?v1, ?v2, "checking collinearity");

            if v1.magnitude() < 1e-10 || v2.magnitude() < 1e-10 {
                debug!("skipping - zero length vector");
                continue;
            }
            let cross_mag = v1.cross(&v2).magnitude();
            debug!(cross_mag, "cross product magnitude");

            if cross_mag > 1e-10 {
                debug!("found three non-collinear points");
                break; // Found three non-collinear points
            }
        }
    }

    if points.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Cannot find three non-collinear vertices".to_string(),
        ));
    }

    // Create plane from three points
    let origin = points[0];
    let v1 = Vector3::from(points[1] - points[0]);
    let v2 = Vector3::from(points[2] - points[0]);

    // Calculate cross product and check if it's non-zero
    let cross = v1.cross(&v2);

    // Log vectors for debugging
    debug!(?v1, ?v2, "planar surface basis vectors");
    debug!(cross_mag = cross.magnitude(), "cross product magnitude");

    if cross.magnitude() < 1e-10 {
        // Try diagonal vectors for quads
        if points.len() >= 4 {
            let v3 = Vector3::from(points[3] - points[1]);
            let v4 = Vector3::from(points[2] - points[0]);
            let cross_diag = v3.cross(&v4);

            debug!(?v3, ?v4, "trying diagonal vectors");
            debug!(
                cross_mag = cross_diag.magnitude(),
                "diagonal cross product magnitude"
            );

            if cross_diag.magnitude() > 1e-10 {
                let normal = cross_diag.normalize().map_err(|e| {
                    OperationError::NumericalError(format!("Normal calculation failed: {:?}", e))
                })?;
                use crate::primitives::surface::Plane;
                let plane = Plane::from_point_normal(origin, normal).map_err(|e| {
                    OperationError::NumericalError(format!("Plane creation failed: {:?}", e))
                })?;
                return Ok(Box::new(plane));
            }
        }

        // Try to find another set of three non-collinear points
        for i in 3..vertices.len() {
            let vertex = model
                .vertices
                .get(vertices[i])
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
            let p = Point3::from(vertex.position);
            let v3 = Vector3::from(p - points[0]);
            let cross2 = v1.cross(&v3);
            if cross2.magnitude() > 1e-10 {
                let normal = cross2.normalize().map_err(|e| {
                    OperationError::NumericalError(format!("Normal calculation failed: {:?}", e))
                })?;
                use crate::primitives::surface::Plane;
                let plane = Plane::from_point_normal(origin, normal).map_err(|e| {
                    OperationError::NumericalError(format!("Plane creation failed: {:?}", e))
                })?;
                return Ok(Box::new(plane));
            }
        }
        return Err(OperationError::InvalidGeometry(
            "All vertices are collinear".to_string(),
        ));
    }

    debug!(?cross, cross_mag = cross.magnitude(), "final cross product");
    let normal = cross.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Normal calculation failed: {:?}", e))
    })?;

    use crate::primitives::surface::Plane;
    let plane = Plane::from_point_normal(origin, normal)
        .map_err(|e| OperationError::NumericalError(format!("Plane creation failed: {:?}", e)))?;
    Ok(Box::new(plane))
}

/// Create a ruled surface type (placeholder for now)
#[derive(Debug, Clone)]
pub struct RuledSurface {
    pub edge1: EdgeId,
    pub edge2: EdgeId,
}

/*
#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::builder::Builder;

    #[test]
    fn test_simple_extrusion() {
        let mut builder = Builder::new();

        // Create a simple rectangular face to extrude
        // This would require setting up vertices, edges, loops, and face
        // For now, this is a placeholder test

        // Test would verify:
        // - Correct number of faces created
        // - Proper orientation of faces
        // - Watertight solid
        // - Correct volume
    }
}
*/
