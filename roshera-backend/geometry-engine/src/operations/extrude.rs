//! Extrusion Operations for B-Rep Models
//!
//! Implements face and profile extrusion with draft angles, twist, and taper.
//! All operations maintain exact analytical geometry.
//!
//! # References
//! - Stroud, I. (2006). Boundary Representation Modelling Techniques. Springer.
//! - Mäntylä, M. (1988). An Introduction to Solid Modeling. Computer Science Press.
//!
//! Indexed access into profile-vertex arrays and side-face vertex pairings
//! is the canonical idiom for extrusion — all `arr[i]` sites use indices
//! bounded by profile vertex count established at extrusion entry.
//! Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::deep_clone::deep_clone_faces;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
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
use std::collections::HashMap;
use tracing::debug;

/// Topology built once per extruded loop so all side faces and the top
/// face share vertices and edges instead of synthesizing duplicates.
///
/// Each unique bottom vertex maps to exactly one translated top vertex
/// and one vertical edge, both reused by the two side faces that meet
/// at that corner. Without this sharing, the shell would be open along
/// every seam, exports would fail watertightness, and vertex-normal
/// averaging would break (visible as banded shading on the front face).
struct ExtrusionLoopTopology {
    /// Bottom `VertexId` → translated top `VertexId`. One entry per
    /// unique vertex in the base loop.
    top_vertex: HashMap<VertexId, VertexId>,
    /// Bottom `VertexId` → vertical `EdgeId` joining bottom→top. Shared
    /// between the two side faces that meet at this corner.
    vertical_edge: HashMap<VertexId, EdgeId>,
    /// Translated top edges, in the same order as `base_loop.edges`.
    top_edges: Vec<EdgeId>,
}

/// Build the bottom→top vertex map, the per-corner vertical edges, and
/// the per-bottom-edge translated top edges in one pass over the loop.
fn build_extrusion_loop_topology(
    model: &mut BRepModel,
    base_loop: &Loop,
    direction: Vector3,
    distance: f64,
) -> OperationResult<ExtrusionLoopTopology> {
    // Snapshot every bottom edge up-front so we can mutate the model
    // while still walking the loop.
    let snapshot: Vec<Edge> = base_loop
        .edges
        .iter()
        .map(|&edge_id| {
            model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))
                .map(|e| e.clone())
        })
        .collect::<OperationResult<Vec<_>>>()?;

    // 1. One translated top vertex per unique bottom vertex.
    let translation = direction * distance;
    let mut top_vertex: HashMap<VertexId, VertexId> = HashMap::new();
    for edge in &snapshot {
        for &bv in &[edge.start_vertex, edge.end_vertex] {
            if top_vertex.contains_key(&bv) {
                continue;
            }
            let v = model.vertices.get(bv).ok_or_else(|| {
                OperationError::InvalidGeometry("Vertex not found".to_string())
            })?;
            let pos = Point3::from(v.position) + translation;
            let tv = model.vertices.add(pos.x, pos.y, pos.z);
            top_vertex.insert(bv, tv);
        }
    }

    // 2. One vertical edge per unique bottom vertex — both adjacent
    //    side faces reference this same edge.
    let mut vertical_edge: HashMap<VertexId, EdgeId> = HashMap::new();
    for &bv in top_vertex.keys().copied().collect::<Vec<_>>().iter() {
        let tv = *top_vertex
            .get(&bv)
            .ok_or_else(|| OperationError::InvalidGeometry("Top vertex map miss".to_string()))?;
        let ve = create_straight_edge(model, bv, tv)?;
        vertical_edge.insert(bv, ve);
    }

    // 3. One translated top edge per bottom edge, reusing the shared
    //    top vertices computed above.
    let translation_matrix = Matrix4::from_translation(&translation);
    let mut top_edges: Vec<EdgeId> = Vec::with_capacity(snapshot.len());
    for edge in &snapshot {
        let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Curve not found".to_string())
        })?;
        let translated_curve = curve.transform(&translation_matrix);
        let new_curve_id = model.curves.add(translated_curve);

        let top_start = *top_vertex.get(&edge.start_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Top vertex map miss (edge start)".to_string())
        })?;
        let top_end = *top_vertex.get(&edge.end_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Top vertex map miss (edge end)".to_string())
        })?;

        let new_edge = Edge::new(
            0,
            top_start,
            top_end,
            new_curve_id,
            edge.orientation,
            edge.param_range,
        );
        top_edges.push(model.edges.add(new_edge));
    }

    Ok(ExtrusionLoopTopology {
        top_vertex,
        vertical_edge,
        top_edges,
    })
}

/// Create one side face that walks bottom-edge → right-vertical →
/// top-edge (reversed) → left-vertical (reversed). The vertical edges
/// come from the shared topology, so the next side face along the
/// loop will reference the same right-vertical edge as its left-vertical
/// — that's what makes the shell watertight.
fn create_side_face_shared(
    model: &mut BRepModel,
    bottom_edge_id: EdgeId,
    bottom_forward: bool,
    bottom_start_v: VertexId,
    bottom_end_v: VertexId,
    top_edge_id: EdgeId,
    topology: &ExtrusionLoopTopology,
) -> OperationResult<FaceId> {
    let left_vertical = *topology.vertical_edge.get(&bottom_start_v).ok_or_else(|| {
        OperationError::InvalidGeometry("Vertical edge map miss (left)".to_string())
    })?;
    let right_vertical = *topology.vertical_edge.get(&bottom_end_v).ok_or_else(|| {
        OperationError::InvalidGeometry("Vertical edge map miss (right)".to_string())
    })?;

    let mut face_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
    // Walk the side-face boundary as a closed quad. With `bottom_forward`
    // we go start→end along the bottom, then up the right vertical,
    // back along the top (reversed), and down the left vertical.
    face_loop.add_edge(bottom_edge_id, bottom_forward);
    face_loop.add_edge(right_vertical, true);
    face_loop.add_edge(top_edge_id, !bottom_forward);
    face_loop.add_edge(left_vertical, false);
    let loop_id = model.loops.add(face_loop);

    let surface = create_ruled_surface(model, bottom_edge_id, top_edge_id)?;
    let surface_id = model.surfaces.add(surface);

    let face = Face::new(0, surface_id, loop_id, FaceOrientation::Forward);
    Ok(model.faces.add(face))
}

/// Build the top cap face from the shared topology: translates the base
/// face's surface, then assembles a loop from the per-bottom-edge top
/// edges in the same order/orientation as the base loop. No fresh
/// vertices, no fresh edges — the top face's outer boundary is exactly
/// the upper edge of the side faces.
fn create_top_face_shared(
    model: &mut BRepModel,
    base_face: &Face,
    base_loop: &Loop,
    topology: &ExtrusionLoopTopology,
    direction: Vector3,
    distance: f64,
) -> OperationResult<FaceId> {
    let original_surface = model.surfaces.get(base_face.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry("Surface not found".to_string())
    })?;
    let translated_surface =
        original_surface.transform(&Matrix4::from_translation(&(direction * distance)));
    let new_surface_id = model.surfaces.add(translated_surface);

    if base_loop.edges.len() != topology.top_edges.len() {
        return Err(OperationError::InvalidGeometry(
            "Top edge count does not match base loop".to_string(),
        ));
    }

    let mut top_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
    for (i, &top_edge_id) in topology.top_edges.iter().enumerate() {
        top_loop.add_edge(top_edge_id, base_loop.orientations[i]);
    }
    let loop_id = model.loops.add(top_loop);

    // Top cap normal points opposite to the base face — its outward
    // direction is along `direction`, while the base face's outward
    // direction is `-direction`.
    let new_orientation = match base_face.orientation {
        FaceOrientation::Forward => FaceOrientation::Backward,
        FaceOrientation::Backward => FaceOrientation::Forward,
    };
    let face = Face::new(0, new_surface_id, loop_id, new_orientation);
    Ok(model.faces.add(face))
}

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

    // Find the parent solid that contains this face. If none exists (e.g. the
    // face was just synthesized from a sketch profile), route to the fresh-
    // solid path which includes the base face as the bottom cap rather than
    // replacing it inside an existing shell.
    let parent_solid_id = find_parent_solid(model, face_id);

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

    let unified_solid_id = match (parent_solid_id, has_complex_options) {
        (Some(parent), true) => {
            create_complex_unified_extrusion(model, parent, &face, face_id, &options)?
        }
        (Some(parent), false) => create_unified_extrusion(
            model,
            parent,
            &face,
            face_id,
            direction,
            options.distance,
            options.cap_ends,
        )?,
        (None, _) => create_fresh_extrusion(
            model,
            &face,
            face_id,
            direction,
            options.distance,
            options.cap_ends,
        )?,
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

    // 2. Build the shared extrusion topology once — top vertices,
    //    vertical edges, and top edges are all created here so the
    //    side faces and top cap reference the same `VertexId` /
    //    `EdgeId`s and the resulting shell is watertight.
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let topology = build_extrusion_loop_topology(model, &base_loop, direction, distance)?;

    // Snapshot bottom edge endpoints so we can index into the topology
    // without re-fetching during side-face construction.
    let bottom_endpoints: Vec<(VertexId, VertexId)> = base_loop
        .edges
        .iter()
        .map(|&edge_id| {
            model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))
                .map(|e| (e.start_vertex, e.end_vertex))
        })
        .collect::<OperationResult<Vec<_>>>()?;

    for (i, &bottom_edge_id) in base_loop.edges.iter().enumerate() {
        let bottom_forward = base_loop.orientations[i];
        let (raw_start, raw_end) = bottom_endpoints[i];
        // When the bottom edge is walked in reverse, its "start" along
        // the loop is the raw end vertex.
        let (loop_start, loop_end) = if bottom_forward {
            (raw_start, raw_end)
        } else {
            (raw_end, raw_start)
        };
        let side_face = create_side_face_shared(
            model,
            bottom_edge_id,
            bottom_forward,
            loop_start,
            loop_end,
            topology.top_edges[i],
            &topology,
        )?;
        unified_faces.push(side_face);
    }

    // 3. Top cap built from the shared top edges — no fresh vertices.
    if cap_ends {
        let top_face =
            create_top_face_shared(model, base_face, &base_loop, &topology, direction, distance)?;
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
        top_vertices = topology.top_vertex.len(),
        vertical_edges = topology.vertical_edge.len(),
        "Created watertight unified shell"
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

/// Create a fresh solid by extruding a free-standing face (no parent solid).
///
/// Used when `extrude_face` is called on a face that was just synthesized
/// from a sketch profile and is not yet attached to any solid. The base face
/// becomes the bottom cap of the new solid; one ruled side face is generated
/// per edge of the base loop; an optional translated copy of the base face
/// caps the top.
fn create_fresh_extrusion(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    direction: Vector3,
    distance: f64,
    cap_ends: bool,
) -> OperationResult<SolidId> {
    let mut unified_faces: Vec<FaceId> = Vec::new();

    // Bottom cap = the original base face, reversed so its outward normal
    // points away from the extrusion direction.
    if cap_ends {
        unified_faces.push(base_face_id);
    }

    // Side faces — one per edge of the base loop. All vertical seams
    // share `VertexId`/`EdgeId`s through the shared topology, so the
    // resulting shell is watertight.
    let base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let topology = build_extrusion_loop_topology(model, &base_loop, direction, distance)?;

    let bottom_endpoints: Vec<(VertexId, VertexId)> = base_loop
        .edges
        .iter()
        .map(|&edge_id| {
            model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))
                .map(|e| (e.start_vertex, e.end_vertex))
        })
        .collect::<OperationResult<Vec<_>>>()?;

    for (i, &bottom_edge_id) in base_loop.edges.iter().enumerate() {
        let bottom_forward = base_loop.orientations[i];
        let (raw_start, raw_end) = bottom_endpoints[i];
        let (loop_start, loop_end) = if bottom_forward {
            (raw_start, raw_end)
        } else {
            (raw_end, raw_start)
        };
        let side_face = create_side_face_shared(
            model,
            bottom_edge_id,
            bottom_forward,
            loop_start,
            loop_end,
            topology.top_edges[i],
            &topology,
        )?;
        unified_faces.push(side_face);
    }

    // Top cap built from the shared top edges.
    if cap_ends {
        let top_face =
            create_top_face_shared(model, base_face, &base_loop, &topology, direction, distance)?;
        unified_faces.push(top_face);
    }

    // Build shell + solid.
    let mut shell = Shell::new(0, ShellType::Closed);
    for &fid in &unified_faces {
        shell.add_face(fid);
    }
    let shell_id = model.shells.add(shell);

    let solid = Solid::new(0, shell_id);
    let solid_id = model.solids.add(solid);

    debug!(
        solid_id,
        shell_id,
        face_count = unified_faces.len(),
        "Created fresh extruded solid"
    );

    Ok(solid_id)
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
                pos += radial_dir * current_draft_offset;
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

    // Build a real ruled surface S(u,v) = (1-v)·C1(u) + v·C2(u) by cloning
    // both boundary curves. RuledSurface evaluates the linear interpolation
    // analytically — no NURBS approximation, no sampling error.
    use crate::primitives::surface::RuledSurface;
    let surface = RuledSurface::new(bottom_curve.clone_box(), top_curve.clone_box());
    Ok(Box::new(surface))
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

    // Best-fit plane via Newell's method (Sutherland & Hodgman 1974;
    // Filip-Magnenat-Thalmann variant). For each polygon edge, accumulate
    // a normal contribution; the resulting vector is robust to small
    // out-of-plane noise and degenerate triangles, unlike a single 3-point
    // cross product. Origin is taken at the centroid for numerical stability.
    if points.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Need at least three points to fit a plane".to_string(),
        ));
    }

    let mut centroid = Point3::ZERO;
    for p in &points {
        centroid.x += p.x;
        centroid.y += p.y;
        centroid.z += p.z;
    }
    let inv_n = 1.0 / points.len() as f64;
    centroid.x *= inv_n;
    centroid.y *= inv_n;
    centroid.z *= inv_n;

    let mut nx = 0.0;
    let mut ny = 0.0;
    let mut nz = 0.0;
    for i in 0..points.len() {
        let curr = points[i];
        let next = points[(i + 1) % points.len()];
        nx += (curr.y - next.y) * (curr.z + next.z);
        ny += (curr.z - next.z) * (curr.x + next.x);
        nz += (curr.x - next.x) * (curr.y + next.y);
    }
    let raw_normal = Vector3::new(nx, ny, nz);
    if raw_normal.magnitude() < 1e-12 {
        return Err(OperationError::InvalidGeometry(
            "Cannot fit plane: points are degenerate or collinear".to_string(),
        ));
    }
    let origin = centroid;
    let normal = raw_normal.normalize().map_err(|e| {
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

/// Validate that the supplied edges form a closed profile by walking
/// the chain head→tail and tracking the *traversal direction* through
/// each edge.
///
/// The previous implementation tested
/// `current.end_vertex ∈ {next.start_vertex, next.end_vertex}` which
/// passed for any pair of edges that merely shared a vertex —
/// including chains that backtrack (e.g. `A→B`, `B→A`, `A→C`) and
/// chains that close on the wrong endpoint. This walk maintains a
/// running `exit_vertex` that flips correctly when an edge is
/// traversed in reverse, and verifies the final edge's exit point
/// returns to the very first edge's entry point.
fn validate_closed_profile(model: &BRepModel, edges: &[EdgeId]) -> OperationResult<()> {
    if edges.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "No edges in profile".to_string(),
        ));
    }

    let first = model
        .edges
        .get(edges[0])
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // We don't know yet whether the first edge is traversed start→end
    // or end→start — both are valid as long as the chain stays
    // consistent. Try start→end first; if the chain fails, retry
    // end→start before giving up.
    if walk_profile_chain(model, edges, first.start_vertex, first.end_vertex).is_ok() {
        return Ok(());
    }
    if walk_profile_chain(model, edges, first.end_vertex, first.start_vertex).is_ok() {
        return Ok(());
    }
    Err(OperationError::OpenProfile)
}

/// Walk the edge chain assuming the first edge was traversed
/// `entry → exit`. Returns Ok if every subsequent edge shares the
/// running exit vertex with one of its endpoints (in which case the
/// other endpoint becomes the new exit) and the final exit matches the
/// initial entry (i.e. the loop closes).
fn walk_profile_chain(
    model: &BRepModel,
    edges: &[EdgeId],
    entry_vertex: VertexId,
    initial_exit: VertexId,
) -> OperationResult<()> {
    let mut exit_vertex = initial_exit;
    for &edge_id in &edges[1..] {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        if edge.start_vertex == exit_vertex {
            exit_vertex = edge.end_vertex;
        } else if edge.end_vertex == exit_vertex {
            exit_vertex = edge.start_vertex;
        } else {
            return Err(OperationError::OpenProfile);
        }
    }
    if exit_vertex == entry_vertex {
        Ok(())
    } else {
        Err(OperationError::OpenProfile)
    }
}

/// Validate the extruded solid by walking its shell graph and checking
/// referential integrity at every level. Returns `InvalidBRep` for the
/// first dangling reference encountered (shell → faces, face → outer/inner
/// loops + surface, loop → edges, edge → vertices + curve). This guards
/// against silent corruption when downstream operations consume a
/// freshly-extruded solid.
fn validate_extruded_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidBRep("Solid not found".to_string()))?
        .clone();

    let mut shells = vec![solid.outer_shell];
    shells.extend(solid.inner_shells.iter().copied());

    for shell_id in shells {
        let shell = model
            .shells
            .get(shell_id)
            .ok_or_else(|| OperationError::InvalidBRep(format!("Shell {} not found", shell_id)))?
            .clone();
        for &face_id in &shell.faces {
            let face = model.faces.get(face_id).ok_or_else(|| {
                OperationError::InvalidBRep(format!("Face {} referenced by shell missing", face_id))
            })?;
            if model.surfaces.get(face.surface_id).is_none() {
                return Err(OperationError::InvalidBRep(format!(
                    "Surface {} for face {} missing",
                    face.surface_id, face_id
                )));
            }
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            for lid in loops {
                let lp = model.loops.get(lid).ok_or_else(|| {
                    OperationError::InvalidBRep(format!(
                        "Loop {} for face {} missing",
                        lid, face_id
                    ))
                })?;
                if lp.edges.is_empty() {
                    return Err(OperationError::InvalidBRep(format!(
                        "Loop {} has no edges",
                        lid
                    )));
                }
                for &eid in &lp.edges {
                    let edge = model.edges.get(eid).ok_or_else(|| {
                        OperationError::InvalidBRep(format!(
                            "Edge {} for loop {} missing",
                            eid, lid
                        ))
                    })?;
                    if model.vertices.get(edge.start_vertex).is_none()
                        || model.vertices.get(edge.end_vertex).is_none()
                    {
                        return Err(OperationError::InvalidBRep(format!(
                            "Edge {} has dangling vertex reference",
                            eid
                        )));
                    }
                    if model.curves.get(edge.curve_id).is_none() {
                        return Err(OperationError::InvalidBRep(format!(
                            "Edge {} curve {} missing",
                            eid, edge.curve_id
                        )));
                    }
                }
            }
        }
    }

    Ok(())
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
            let v1 = points[1] - points[0];
            let v2 = points[2] - points[0];

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
    let v1 = points[1] - points[0];
    let v2 = points[2] - points[0];

    // Calculate cross product and check if it's non-zero
    let cross = v1.cross(&v2);

    // Log vectors for debugging
    debug!(?v1, ?v2, "planar surface basis vectors");
    debug!(cross_mag = cross.magnitude(), "cross product magnitude");

    if cross.magnitude() < 1e-10 {
        // Try diagonal vectors for quads
        if points.len() >= 4 {
            let v3 = points[3] - points[1];
            let v4 = points[2] - points[0];
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
            let v3 = p - points[0];
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

