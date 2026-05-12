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
    curve::{Arc, Circle, Curve},
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    surface::{Cylinder, Surface},
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
            let v = model
                .vertices
                .get(bv)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
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
        let curve = model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
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
///
/// `inner_specs` carries one `(inner_base_loop, inner_topology)` pair per
/// inner loop on the base face. Each pair produces a matching inner loop
/// on the top cap so the bottom and top caps have identical hole
/// topology. Pass an empty slice when the base face has no inner loops.
fn create_top_face_shared(
    model: &mut BRepModel,
    base_face: &Face,
    base_loop: &Loop,
    topology: &ExtrusionLoopTopology,
    inner_specs: &[(Loop, ExtrusionLoopTopology)],
    direction: Vector3,
    distance: f64,
) -> OperationResult<FaceId> {
    let original_surface = model
        .surfaces
        .get(base_face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
    let translated_surface =
        original_surface.transform(&Matrix4::from_translation(&(direction * distance)));
    // Pick the orientation that lifts the (possibly winding-dependent)
    // surface normal to +direction — the outward direction for the top
    // cap. Computed before the surface is moved into the store so we
    // don't need a second lookup.
    let new_orientation = orientation_for_target(translated_surface.as_ref(), direction)?;
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
    let outer_loop_id = model.loops.add(top_loop);

    // One inner loop on the top cap per hole on the base face. The hole's
    // top edges sit in the same order/orientation as the corresponding
    // bottom edges (shared via the inner topology), so the cap's hole
    // boundary matches the upper edge of the inner-loop side faces.
    let mut inner_loop_ids: Vec<crate::primitives::r#loop::LoopId> =
        Vec::with_capacity(inner_specs.len());
    for (inner_base_loop, inner_topology) in inner_specs {
        if inner_base_loop.edges.len() != inner_topology.top_edges.len() {
            return Err(OperationError::InvalidGeometry(
                "Top edge count does not match inner loop".to_string(),
            ));
        }
        let mut inner_top_loop = Loop::new(0, crate::primitives::r#loop::LoopType::Inner);
        for (i, &top_edge_id) in inner_topology.top_edges.iter().enumerate() {
            inner_top_loop.add_edge(top_edge_id, inner_base_loop.orientations[i]);
        }
        inner_loop_ids.push(model.loops.add(inner_top_loop));
    }

    let mut face =
        Face::with_capacity(0, new_surface_id, outer_loop_id, new_orientation, inner_loop_ids.len());
    for loop_id in inner_loop_ids {
        face.add_inner_loop(loop_id);
    }
    Ok(model.faces.add(face))
}

/// Pick the `FaceOrientation` that aligns the face's oriented outward
/// normal (`surface_normal × orientation.sign()`) with `target`.
///
/// Required because `create_face_from_profile` (and other profile-from-
/// edges builders) always emit `FaceOrientation::Forward` while the
/// underlying Newell's-method plane normal direction depends on the
/// polygon's winding — CCW polygons get a `+Z` normal, CW polygons
/// get `-Z`. Without this fix, the bottom cap of a CCW + +Z extrusion
/// has its oriented normal pointing INTO the solid, which inverts the
/// signed dihedral at every rim edge and makes fillet / chamfer
/// remove material from the wrong side. The cylinder primitive avoids
/// this by hand-building its caps with `Plane::from_point_normal(_,
/// -axis)` / `+axis`; this helper is the same fix factored as a
/// reusable post-build adjustment.
///
/// Sampled at the surface's parametric midpoint — sufficient for the
/// planar caps this is currently called on. If the target and surface
/// normal are exactly perpendicular at the sample point (oblique
/// extrusion edge case) the helper prefers `Forward` deterministically.
fn orientation_for_target(
    surface: &dyn Surface,
    target: Vector3,
) -> OperationResult<FaceOrientation> {
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let u_mid = 0.5 * (u_min + u_max);
    let v_mid = 0.5 * (v_min + v_max);
    let n = surface
        .normal_at(u_mid, v_mid)
        .map_err(|e| OperationError::NumericalError(format!("Surface normal failed: {:?}", e)))?;
    if n.dot(&target) >= 0.0 {
        Ok(FaceOrientation::Forward)
    } else {
        Ok(FaceOrientation::Backward)
    }
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

    // The unified-extrusion path replaces `outer_shell` on the parent solid
    // in-place (see create_unified_extrusion / create_complex_unified_extrusion).
    // Any volume/area/inertia previously memoised on the Solid is stale.
    if let Some(solid) = model.solids.get_mut(unified_solid_id) {
        solid.invalidate_mass_props_cache();
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
            .with_input_faces([face_id as u64])
            .with_output_solids([unified_solid_id as u64]),
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

    // 2. Build the shared extrusion topology once per loop — top
    //    vertices, vertical edges, and top edges are all created here so
    //    the side faces and top cap reference the same `VertexId` /
    //    `EdgeId`s and the resulting shell is watertight. The same
    //    treatment applies to each inner loop (hole) on the base face;
    //    see `create_fresh_extrusion` for the doc on inner-loop winding.
    validate_inner_loops_inside_outer(model, base_face)?;

    let outer_base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let outer_topology =
        build_extrusion_loop_topology(model, &outer_base_loop, direction, distance)?;

    build_loop_side_faces(
        model,
        &outer_base_loop,
        &outer_topology,
        &mut unified_faces,
    )?;

    let inner_loop_ids = base_face.inner_loops.clone();
    let mut inner_specs: Vec<(Loop, ExtrusionLoopTopology)> =
        Vec::with_capacity(inner_loop_ids.len());
    for inner_loop_id in inner_loop_ids {
        let inner_loop = model
            .loops
            .get(inner_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Inner loop not found".to_string()))?
            .clone();
        let inner_topology =
            build_extrusion_loop_topology(model, &inner_loop, direction, distance)?;
        build_loop_side_faces(model, &inner_loop, &inner_topology, &mut unified_faces)?;
        inner_specs.push((inner_loop, inner_topology));
    }

    // 3. Top cap built from the shared top edges — no fresh vertices.
    if cap_ends {
        let top_face = create_top_face_shared(
            model,
            base_face,
            &outer_base_loop,
            &outer_topology,
            &inner_specs,
            direction,
            distance,
        )?;
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
        top_vertices = outer_topology.top_vertex.len(),
        vertical_edges = outer_topology.vertical_edge.len(),
        inner_loops = inner_specs.len(),
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
/// per edge of the outer loop; an optional translated copy of the base face
/// caps the top.
///
/// **Multi-loop support:** when `base_face.inner_loops` is non-empty, each
/// inner loop (hole) is treated the same way as the outer loop — one ruled
/// side face per inner-loop edge, and the top cap is built with matching
/// inner loops. The inner-loop winding (CW when the outer is CCW per the
/// B-Rep invariant) naturally orients the side faces' outward normals into
/// the hole, so material lies between the outer-loop side walls and the
/// inner-loop side walls. See `validate_inner_loops_inside_outer` for the
/// containment guard.
fn create_fresh_extrusion(
    model: &mut BRepModel,
    base_face: &Face,
    base_face_id: FaceId,
    direction: Vector3,
    distance: f64,
    cap_ends: bool,
) -> OperationResult<SolidId> {
    // Reject malformed input up front: every inner loop must lie inside
    // the outer loop, otherwise the extruded shell would be degenerate
    // (e.g. side faces crossing each other in 3D).
    validate_inner_loops_inside_outer(model, base_face)?;

    let mut unified_faces: Vec<FaceId> = Vec::new();

    // Bottom cap = the original base face, with its orientation set so
    // the oriented outward normal points opposite to `direction` (i.e.
    // away from the extrusion volume). `create_face_from_profile`
    // always emits `FaceOrientation::Forward`, but Newell's-method
    // plane normals follow the polygon winding — so the correct
    // orientation depends on whether the base surface normal points
    // along or against `direction`. The base face already references
    // its inner loops by value, so the bottom cap is multi-loop
    // automatically without any extra work here.
    if cap_ends {
        // Scope the immutable borrow on `model.surfaces` so the
        // subsequent mutable borrow on `model.faces` is unambiguous to
        // the borrow checker — even though the two stores are disjoint
        // fields of `BRepModel`.
        let correct_orientation = {
            let base_surface = model
                .surfaces
                .get(base_face.surface_id)
                .ok_or_else(|| {
                    OperationError::InvalidGeometry("Base surface not found".to_string())
                })?;
            orientation_for_target(base_surface, -direction)?
        };
        if let Some(face_mut) = model.faces.get_mut(base_face_id) {
            face_mut.orientation = correct_orientation;
        }
        unified_faces.push(base_face_id);
    }

    // Outer side faces — one per edge of the outer loop. All vertical
    // seams share `VertexId`/`EdgeId`s through the shared topology, so the
    // resulting shell is watertight along the outer boundary.
    let outer_base_loop = model
        .loops
        .get(base_face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let outer_topology =
        build_extrusion_loop_topology(model, &outer_base_loop, direction, distance)?;

    build_loop_side_faces(
        model,
        &outer_base_loop,
        &outer_topology,
        &mut unified_faces,
    )?;

    // Inner-loop side faces — one per edge of each inner loop. The same
    // helper builds them; the inner loop's own CW winding (relative to
    // the outer's CCW) flips the side-face normal so it points into the
    // hole, which is what we want for the wall facing vacuum.
    let inner_loop_ids = base_face.inner_loops.clone();
    let mut inner_specs: Vec<(Loop, ExtrusionLoopTopology)> =
        Vec::with_capacity(inner_loop_ids.len());
    for inner_loop_id in inner_loop_ids {
        let inner_loop = model
            .loops
            .get(inner_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Inner loop not found".to_string()))?
            .clone();
        let inner_topology =
            build_extrusion_loop_topology(model, &inner_loop, direction, distance)?;
        build_loop_side_faces(model, &inner_loop, &inner_topology, &mut unified_faces)?;
        inner_specs.push((inner_loop, inner_topology));
    }

    // Top cap built from the shared top edges — both outer and inner.
    if cap_ends {
        let top_face = create_top_face_shared(
            model,
            base_face,
            &outer_base_loop,
            &outer_topology,
            &inner_specs,
            direction,
            distance,
        )?;
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
        inner_loops = inner_specs.len(),
        "Created fresh extruded solid"
    );

    Ok(solid_id)
}

/// Build one side face per edge of `base_loop`, pushing each new `FaceId`
/// onto `out_faces`. Shared between outer and inner loop extrusion paths.
fn build_loop_side_faces(
    model: &mut BRepModel,
    base_loop: &Loop,
    topology: &ExtrusionLoopTopology,
    out_faces: &mut Vec<FaceId>,
) -> OperationResult<()> {
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

    if base_loop.edges.len() != topology.top_edges.len() {
        return Err(OperationError::InvalidGeometry(
            "Top edge count does not match base loop".to_string(),
        ));
    }
    if base_loop.edges.len() != base_loop.orientations.len() {
        return Err(OperationError::InvalidGeometry(
            "Loop orientations length mismatch".to_string(),
        ));
    }

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
            topology,
        )?;
        out_faces.push(side_face);
    }

    Ok(())
}

/// Validate that every inner loop of `base_face` lies inside the outer
/// loop. Uses each inner loop's vertex centroid as the sample point and
/// `Loop::contains_point` (winding-number) on the outer loop, projected
/// in the dominant-axis plane of the face's surface normal.
///
/// **Limitation:** the centroid may fall outside a highly concave inner
/// loop. For the production case (rectangles, circles approximated as
/// regular polygons, simple polygons emitted by `detect_regions`), all
/// loops are star-shaped relative to their centroid and the test is
/// reliable. Pathological non-star-shaped inner loops can produce false
/// rejections — refine the sampling strategy if that case ever arises.
fn validate_inner_loops_inside_outer(
    model: &BRepModel,
    base_face: &Face,
) -> OperationResult<()> {
    if base_face.inner_loops.is_empty() {
        return Ok(());
    }

    let outer_loop = model.loops.get(base_face.outer_loop).ok_or_else(|| {
        OperationError::InvalidGeometry("Outer loop not found during inner-loop validation".to_string())
    })?;

    let surface = model.surfaces.get(base_face.surface_id).ok_or_else(|| {
        OperationError::InvalidGeometry("Surface not found during inner-loop validation".to_string())
    })?;
    let (u_bounds, v_bounds) = surface.parameter_bounds();
    let u_mid = 0.5 * (u_bounds.0 + u_bounds.1);
    let v_mid = 0.5 * (v_bounds.0 + v_bounds.1);
    let surface_normal = surface
        .normal_at(u_mid, v_mid)
        .map_err(|e| OperationError::NumericalError(format!("Surface normal failed: {:?}", e)))?;

    for &inner_loop_id in &base_face.inner_loops {
        let inner_loop = model
            .loops
            .get(inner_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Inner loop not found".to_string()))?;

        let inner_vertex_ids = inner_loop
            .vertices_cached(&model.edges)
            .map_err(|e| OperationError::NumericalError(format!("Inner loop vertex query failed: {:?}", e)))?;
        if inner_vertex_ids.is_empty() {
            return Err(OperationError::InvalidGeometry(
                "Inner loop has no vertices".to_string(),
            ));
        }

        let mut cx = 0.0;
        let mut cy = 0.0;
        let mut cz = 0.0;
        for vid in &inner_vertex_ids {
            let v = model.vertices.get(*vid).ok_or_else(|| {
                OperationError::InvalidGeometry("Inner-loop vertex not found".to_string())
            })?;
            cx += v.position[0];
            cy += v.position[1];
            cz += v.position[2];
        }
        let n = inner_vertex_ids.len() as f64;
        let centroid = Point3::new(cx / n, cy / n, cz / n);

        let inside = outer_loop
            .contains_point(&centroid, &surface_normal, &model.vertices, &model.edges)
            .map_err(|e| {
                OperationError::NumericalError(format!("Inner-loop containment test failed: {:?}", e))
            })?;
        if !inside {
            return Err(OperationError::InvalidGeometry(format!(
                "Inner loop {} centroid is not inside the outer loop",
                inner_loop_id
            )));
        }
    }

    Ok(())
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

    // Detect the canonical "extruded full-circle" case (e.g. extruding a
    // cylinder cap, the bottom face of a cone, or any closed-disk profile)
    // and emit a Cylinder primitive instead of a generic RuledSurface.
    //
    // Why: the 4-edge "cut rectangle" loop produced by `create_side_face_shared`
    // for a closed-circle base (start_vertex == end_vertex, both vertical
    // seam edges collapsed to the same EdgeId) matches `cylinder_primitive`'s
    // lateral-face topology exactly. RuledSurface would route to
    // `tessellate_generic_face`, whose UV-grid + winding-number test fails
    // on closed-circle boundaries because the 2D loop projection wraps the
    // parameter space — the side face emits zero triangles and the user
    // sees the cone/disk caps but no cylindrical wall between them.
    // Cylinder routes to `tessellate_cylindrical_face`, which knows the
    // seam convention and tessellates correctly.
    if let Some(cyl) = try_build_cylinder_from_circles(bottom_curve, top_curve) {
        return Ok(Box::new(cyl));
    }

    // Build a real ruled surface S(u,v) = (1-v)·C1(u) + v·C2(u) by cloning
    // both boundary curves. RuledSurface evaluates the linear interpolation
    // analytically — no NURBS approximation, no sampling error.
    use crate::primitives::surface::RuledSurface;
    let surface = RuledSurface::new(bottom_curve.clone_box(), top_curve.clone_box());
    Ok(Box::new(surface))
}

/// Extract `(center, axis, radius)` from a curve if it represents a full
/// circle. Handles both `Circle` (which wraps `Arc` internally) and `Arc`
/// with `sweep_angle ≈ 2π`. Returns None for partial arcs, lines, NURBS,
/// or anything else.
fn full_circle_params(curve: &dyn Curve) -> Option<(Point3, Vector3, f64)> {
    use crate::math::consts;
    let any = curve.as_any();
    if let Some(c) = any.downcast_ref::<Circle>() {
        return Some((c.center(), c.normal(), c.radius()));
    }
    if let Some(a) = any.downcast_ref::<Arc>() {
        if (a.sweep_angle.abs() - consts::TWO_PI).abs() < 1e-9 {
            return Some((a.center, a.normal, a.radius));
        }
    }
    None
}

/// If the bottom and top curves are coaxial full circles of equal radius,
/// build a finite `Cylinder` whose axis goes from the bottom center to
/// the top center. Returns None if the geometry doesn't match — the
/// caller falls back to a generic ruled surface.
fn try_build_cylinder_from_circles(
    bottom_curve: &dyn Curve,
    top_curve: &dyn Curve,
) -> Option<Cylinder> {
    let (b_center, b_axis, b_radius) = full_circle_params(bottom_curve)?;
    let (t_center, t_axis, t_radius) = full_circle_params(top_curve)?;

    // Same radius (numerically). Different radii would be a cone, which
    // we don't synthesise here — a generic ruled surface still draws
    // correctly for cones because the cone tessellator isn't involved.
    if (b_radius - t_radius).abs() > 1e-9 {
        return None;
    }

    // Axes must be parallel (translation along axis is the only transform
    // build_extrusion_loop_topology applies, so this is essentially
    // always true for our caller — but verifying is cheap).
    let b_axis_n = b_axis.normalize().ok()?;
    let t_axis_n = t_axis.normalize().ok()?;
    if b_axis_n.dot(&t_axis_n).abs() < 1.0 - 1e-9 {
        return None;
    }

    // The line from bottom center to top center must be parallel to the
    // axis. Otherwise the boundary circles aren't coaxial and a cylinder
    // doesn't fit (e.g. an oblique extrusion of a circle).
    let centers = t_center - b_center;
    let centers_len = centers.magnitude();
    if centers_len < 1e-12 {
        // Degenerate (zero-height) extrusion — shouldn't happen because
        // distance is validated upstream, but bail safely if it does.
        return None;
    }
    let centers_n = centers.normalize().ok()?;
    if centers_n.dot(&b_axis_n).abs() < 1.0 - 1e-9 {
        return None;
    }

    // Height is the signed projection onto the cylinder axis. Negative
    // distances are valid (the user pulled in -axis direction); the
    // Cylinder's height_limits = [0, height] still bounds the lateral
    // surface as long as height > 0 — flip when needed so the origin
    // sits at whichever circle is "lower" along the axis.
    let signed_height = centers.dot(&b_axis_n);
    let (origin, height) = if signed_height >= 0.0 {
        (b_center, signed_height)
    } else {
        (t_center, -signed_height)
    };

    Cylinder::new_finite(origin, b_axis_n, b_radius, height).ok()
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::Line;
    use crate::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

    /// Add a Line curve + Edge between two existing vertices.
    fn add_line_edge(model: &mut BRepModel, v_start: VertexId, v_end: VertexId) -> EdgeId {
        let s = model.vertices.get(v_start).expect("start vertex");
        let e = model.vertices.get(v_end).expect("end vertex");
        let line = Line::new(Point3::from(s.position), Point3::from(e.position));
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
        model.edges.add(edge)
    }

    /// Closed CCW rectangular profile in XY at z=0. Returns the four
    /// edges in bottom→right→top→left order.
    fn make_rectangle(model: &mut BRepModel, width: f64, height: f64) -> Vec<EdgeId> {
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(width, 0.0, 0.0);
        let v2 = model.vertices.add(width, height, 0.0);
        let v3 = model.vertices.add(0.0, height, 0.0);
        vec![
            add_line_edge(model, v0, v1),
            add_line_edge(model, v1, v2),
            add_line_edge(model, v2, v3),
            add_line_edge(model, v3, v0),
        ]
    }

    /// Pick the first face on the box's outer shell.
    fn first_box_face(model: &BRepModel, solid_id: SolidId) -> FaceId {
        let solid = model.solids.get(solid_id).expect("box solid");
        let shell = model.shells.get(solid.outer_shell).expect("outer shell");
        *shell.faces.first().expect("box has faces")
    }

    // -------------------------------------------------------------------
    // ExtrudeOptions defaults
    // -------------------------------------------------------------------

    #[test]
    fn extrude_options_default_values_match_documentation() {
        let opts = ExtrudeOptions::default();
        assert_eq!(opts.direction, Vector3::Z);
        assert!((opts.distance - 1.0).abs() < 1e-12);
        assert!(!opts.symmetric);
        assert_eq!(opts.draft_angle, 0.0);
        assert_eq!(opts.twist_angle, 0.0);
        assert!(opts.cap_ends);
        assert!((opts.end_scale - 1.0).abs() < 1e-12);
    }

    // -------------------------------------------------------------------
    // create_face_from_profile
    // -------------------------------------------------------------------

    #[test]
    fn create_face_from_rectangle_profile_succeeds() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 10.0, 5.0);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        assert!(model.faces.get(face_id).is_some());
    }

    #[test]
    fn create_face_from_open_profile_is_error() {
        let mut model = BRepModel::new();
        // Open chain: only two edges.
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(1.0, 1.0, 0.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
        ];
        let result = create_face_from_profile(&mut model, edges);
        assert!(result.is_err());
    }

    #[test]
    fn create_face_from_empty_profile_is_error() {
        let mut model = BRepModel::new();
        let result = create_face_from_profile(&mut model, vec![]);
        assert!(result.is_err());
    }

    // -------------------------------------------------------------------
    // extrude_profile happy paths
    // -------------------------------------------------------------------

    #[test]
    fn extrude_rectangle_profile_creates_solid_with_six_faces() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 10.0, 5.0);
        let opts = ExtrudeOptions {
            distance: 4.0,
            ..Default::default()
        };
        let solid_id = extrude_profile(&mut model, edges, opts).expect("extrude");
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        // 4 sides + bottom cap + top cap.
        assert_eq!(shell.faces.len(), 6);
    }

    #[test]
    fn extrude_rectangle_profile_along_negative_distance_succeeds() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 2.0, 2.0);
        let opts = ExtrudeOptions {
            distance: -3.0,
            ..Default::default()
        };
        let result = extrude_profile(&mut model, edges, opts);
        assert!(result.is_ok());
    }

    #[test]
    fn extrude_l_shape_profile_creates_solid_with_eight_faces() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(6.0, 0.0, 0.0);
        let v2 = model.vertices.add(6.0, 2.0, 0.0);
        let v3 = model.vertices.add(2.0, 2.0, 0.0);
        let v4 = model.vertices.add(2.0, 4.0, 0.0);
        let v5 = model.vertices.add(0.0, 4.0, 0.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
            add_line_edge(&mut model, v2, v3),
            add_line_edge(&mut model, v3, v4),
            add_line_edge(&mut model, v4, v5),
            add_line_edge(&mut model, v5, v0),
        ];
        let opts = ExtrudeOptions {
            distance: 2.0,
            ..Default::default()
        };
        let solid_id = extrude_profile(&mut model, edges, opts).expect("extrude L");
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        assert_eq!(shell.faces.len(), 8);
    }

    #[test]
    fn extrude_along_x_axis_succeeds() {
        let mut model = BRepModel::new();
        // Profile in YZ plane → extrude along +X.
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(0.0, 1.0, 0.0);
        let v2 = model.vertices.add(0.0, 1.0, 1.0);
        let v3 = model.vertices.add(0.0, 0.0, 1.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
            add_line_edge(&mut model, v2, v3),
            add_line_edge(&mut model, v3, v0),
        ];
        let opts = ExtrudeOptions {
            direction: Vector3::X,
            distance: 5.0,
            ..Default::default()
        };
        let result = extrude_profile(&mut model, edges, opts);
        assert!(result.is_ok());
    }

    // -------------------------------------------------------------------
    // extrude_face on a box face (parent-solid / unified path)
    // -------------------------------------------------------------------

    #[test]
    fn extrude_face_on_existing_box_returns_solid() {
        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder.create_box_3d(2.0, 2.0, 2.0).expect("box") {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            }
        };
        let face_id = first_box_face(&model, solid_id);
        let opts = ExtrudeOptions {
            distance: 1.0,
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            ..Default::default()
        };
        let result = extrude_face(&mut model, face_id, opts);
        assert!(result.is_ok(), "extrude_face on box face: {:?}", result);
    }

    // -------------------------------------------------------------------
    // Validation errors
    // -------------------------------------------------------------------

    #[test]
    fn extrude_face_rejects_unknown_face_id() {
        let mut model = BRepModel::new();
        let opts = ExtrudeOptions::default();
        let result = extrude_face(&mut model, 99_999, opts);
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(msg.contains("Face not found"), "msg = {msg}");
            }
            other => panic!("expected InvalidGeometry, got {other:?}"),
        }
    }

    #[test]
    fn extrude_face_rejects_zero_distance() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 1.0, 1.0);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = ExtrudeOptions {
            distance: 0.0,
            ..Default::default()
        };
        let result = extrude_face(&mut model, face_id, opts);
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(msg.contains("distance"), "msg = {msg}");
            }
            other => panic!("expected distance error, got {other:?}"),
        }
    }

    #[test]
    fn extrude_face_rejects_zero_direction() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 1.0, 1.0);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = ExtrudeOptions {
            direction: Vector3::ZERO,
            distance: 1.0,
            ..Default::default()
        };
        let result = extrude_face(&mut model, face_id, opts);
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(msg.contains("direction"), "msg = {msg}");
            }
            other => panic!("expected direction error, got {other:?}"),
        }
    }

    #[test]
    fn extrude_face_rejects_draft_angle_at_or_above_ninety_degrees() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 1.0, 1.0);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        let opts = ExtrudeOptions {
            distance: 1.0,
            draft_angle: std::f64::consts::FRAC_PI_2,
            ..Default::default()
        };
        let result = extrude_face(&mut model, face_id, opts);
        match result {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(msg.contains("Draft angle"), "msg = {msg}");
            }
            other => panic!("expected draft-angle error, got {other:?}"),
        }
    }

    // -------------------------------------------------------------------
    // walk_profile_chain / validate_closed_profile
    // -------------------------------------------------------------------

    #[test]
    fn validate_closed_profile_accepts_rectangle() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 1.0, 1.0);
        assert!(validate_closed_profile(&model, &edges).is_ok());
    }

    #[test]
    fn validate_closed_profile_rejects_open_chain() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(2.0, 0.0, 0.0);
        let edges = vec![
            add_line_edge(&mut model, v0, v1),
            add_line_edge(&mut model, v1, v2),
        ];
        let result = validate_closed_profile(&model, &edges);
        assert!(matches!(result, Err(OperationError::OpenProfile)));
    }

    #[test]
    fn validate_closed_profile_rejects_disconnected_edges() {
        let mut model = BRepModel::new();
        let a0 = model.vertices.add(0.0, 0.0, 0.0);
        let a1 = model.vertices.add(1.0, 0.0, 0.0);
        let b0 = model.vertices.add(5.0, 5.0, 0.0);
        let b1 = model.vertices.add(6.0, 5.0, 0.0);
        let edges = vec![
            add_line_edge(&mut model, a0, a1),
            add_line_edge(&mut model, b0, b1),
        ];
        let result = validate_closed_profile(&model, &edges);
        assert!(matches!(result, Err(OperationError::OpenProfile)));
    }

    #[test]
    fn validate_closed_profile_accepts_reverse_orientation_first_edge() {
        // First edge oriented v1→v0; chain must retry with end→start.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(0.0, 1.0, 0.0);
        let e0 = add_line_edge(&mut model, v1, v0);
        let e1 = add_line_edge(&mut model, v0, v2);
        let e2 = add_line_edge(&mut model, v2, v1);
        let edges = vec![e0, e1, e2];
        assert!(validate_closed_profile(&model, &edges).is_ok());
    }

    #[test]
    fn validate_closed_profile_rejects_empty_input() {
        let model = BRepModel::new();
        let result = validate_closed_profile(&model, &[]);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    // -------------------------------------------------------------------
    // try_build_cylinder_from_circles
    // -------------------------------------------------------------------

    #[test]
    fn try_build_cylinder_returns_none_for_lines() {
        let line_a = Line::new(Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        let line_b = Line::new(Point3::new(0.0, 0.0, 1.0), Point3::new(1.0, 0.0, 1.0));
        assert!(try_build_cylinder_from_circles(&line_a, &line_b).is_none());
    }

    #[test]
    fn try_build_cylinder_from_coaxial_full_circles_succeeds() {
        use crate::primitives::curve::Circle;
        let bottom = Circle::new(Point3::ORIGIN, Vector3::Z, 2.0).expect("bottom");
        let top = Circle::new(Point3::new(0.0, 0.0, 5.0), Vector3::Z, 2.0).expect("top");
        let cyl = try_build_cylinder_from_circles(&bottom, &top).expect("cylinder");
        let limits = cyl.height_limits.expect("height limits set on finite cyl");
        assert!((limits[0] - 0.0).abs() < 1e-12, "limits = {:?}", limits);
        assert!((limits[1] - 5.0).abs() < 1e-9, "limits = {:?}", limits);
        assert!((cyl.radius - 2.0).abs() < 1e-12);
    }

    #[test]
    fn try_build_cylinder_returns_none_for_mismatched_radii() {
        use crate::primitives::curve::Circle;
        let bottom = Circle::new(Point3::ORIGIN, Vector3::Z, 2.0).expect("bottom");
        let top = Circle::new(Point3::new(0.0, 0.0, 5.0), Vector3::Z, 3.0).expect("top");
        assert!(try_build_cylinder_from_circles(&bottom, &top).is_none());
    }

    #[test]
    fn try_build_cylinder_returns_none_for_non_parallel_axes() {
        use crate::primitives::curve::Circle;
        let bottom = Circle::new(Point3::ORIGIN, Vector3::Z, 2.0).expect("bottom");
        let top = Circle::new(Point3::new(0.0, 0.0, 5.0), Vector3::X, 2.0).expect("top");
        assert!(try_build_cylinder_from_circles(&bottom, &top).is_none());
    }

    #[test]
    fn try_build_cylinder_returns_none_for_zero_height() {
        use crate::primitives::curve::Circle;
        let bottom = Circle::new(Point3::ORIGIN, Vector3::Z, 2.0).expect("bottom");
        let top = Circle::new(Point3::ORIGIN, Vector3::Z, 2.0).expect("top");
        assert!(try_build_cylinder_from_circles(&bottom, &top).is_none());
    }

    // -------------------------------------------------------------------
    // full_circle_params
    // -------------------------------------------------------------------

    #[test]
    fn full_circle_params_recognises_circle() {
        use crate::primitives::curve::Circle;
        let c = Circle::new(Point3::new(1.0, 2.0, 3.0), Vector3::Y, 4.0).expect("circle");
        let (center, axis, radius) = full_circle_params(&c).expect("params");
        assert!((center.x - 1.0).abs() < 1e-12);
        assert!((center.y - 2.0).abs() < 1e-12);
        assert!((center.z - 3.0).abs() < 1e-12);
        assert!((axis.normalize().expect("axis").y - 1.0).abs() < 1e-9);
        assert!((radius - 4.0).abs() < 1e-12);
    }

    #[test]
    fn full_circle_params_returns_none_for_line() {
        let line = Line::new(Point3::ORIGIN, Point3::new(1.0, 0.0, 0.0));
        assert!(full_circle_params(&line).is_none());
    }

    #[test]
    fn full_circle_params_returns_none_for_partial_arc() {
        use crate::primitives::curve::Arc;
        let arc =
            Arc::new(Point3::ORIGIN, Vector3::Z, 1.0, 0.0, std::f64::consts::PI).expect("arc");
        assert!(full_circle_params(&arc).is_none());
    }

    // -------------------------------------------------------------------
    // find_parent_solid
    // -------------------------------------------------------------------

    #[test]
    fn find_parent_solid_returns_none_for_orphan_face() {
        let mut model = BRepModel::new();
        let edges = make_rectangle(&mut model, 1.0, 1.0);
        let face_id = create_face_from_profile(&mut model, edges).expect("face");
        assert!(find_parent_solid(&model, face_id).is_none());
    }

    #[test]
    fn find_parent_solid_finds_box_face_owner() {
        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder.create_box_3d(1.0, 1.0, 1.0).expect("box") {
                GeometryId::Solid(id) => id,
                other => panic!("expected solid, got {other:?}"),
            }
        };
        let face_id = first_box_face(&model, solid_id);
        assert_eq!(find_parent_solid(&model, face_id), Some(solid_id));
    }

    // -------------------------------------------------------------------
    // create_straight_edge
    // -------------------------------------------------------------------

    #[test]
    fn create_straight_edge_links_two_vertices() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(2.0, 0.0, 0.0);
        let edge_id = create_straight_edge(&mut model, v0, v1).expect("edge");
        let edge = model.edges.get(edge_id).expect("stored edge");
        assert_eq!(edge.start_vertex, v0);
        assert_eq!(edge.end_vertex, v1);
    }

    #[test]
    fn create_straight_edge_rejects_unknown_vertex() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let result = create_straight_edge(&mut model, v0, 99_999);
        assert!(matches!(
            result,
            Err(OperationError::InvalidGeometry(_))
        ));
    }
}
