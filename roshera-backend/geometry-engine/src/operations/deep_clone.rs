//! Deep geometry cloning for B-Rep models
//!
//! Provides functionality to create complete copies of geometric entities
//! with proper ID remapping for all references.
//!
//! Indexed access into ID-remap tables is the canonical idiom — bounded by
//! source-topology length. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{OperationError, OperationResult};
use crate::math::{Point3, Vector3};
use crate::primitives::{
    curve::CurveId,
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::{Loop, LoopId},
    shell::{Shell, ShellId},
    solid::{Solid, SolidId},
    surface::SurfaceId,
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::HashMap;

/// Context for tracking ID mappings during deep cloning
#[derive(Debug, Default)]
pub struct CloneContext {
    pub vertex_map: HashMap<VertexId, VertexId>,
    pub curve_map: HashMap<CurveId, CurveId>,
    pub edge_map: HashMap<EdgeId, EdgeId>,
    pub loop_map: HashMap<LoopId, LoopId>,
    pub surface_map: HashMap<SurfaceId, SurfaceId>,
    pub face_map: HashMap<FaceId, FaceId>,
    pub shell_map: HashMap<ShellId, ShellId>,
}

impl CloneContext {
    /// Create a new empty clone context
    pub fn new() -> Self {
        Self::default()
    }
}

/// Deep clone vertices with optional position offset
pub fn clone_vertices(
    model: &mut BRepModel,
    vertex_ids: &[VertexId],
    offset: Option<Vector3>,
    context: &mut CloneContext,
) -> OperationResult<Vec<VertexId>> {
    let mut new_ids = Vec::new();

    for &vertex_id in vertex_ids {
        let vertex = model
            .vertices
            .get(vertex_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

        let position = Point3::from(vertex.position);
        let new_position = if let Some(off) = offset {
            position + off
        } else {
            position
        };

        // Force creation of new vertex even if position exists
        // This is necessary for proper deep cloning
        let new_id = model
            .vertices
            .add(new_position.x, new_position.y, new_position.z);

        context.vertex_map.insert(vertex_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone curves
pub fn clone_curves(
    model: &mut BRepModel,
    curve_ids: &[CurveId],
    context: &mut CloneContext,
) -> OperationResult<Vec<CurveId>> {
    let mut new_ids = Vec::new();

    for &curve_id in curve_ids {
        // Use existing clone_curve method
        let new_id = model
            .curves
            .clone_curve(curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Failed to clone curve".to_string()))?;

        context.curve_map.insert(curve_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone edges with remapped vertex and curve references
pub fn clone_edges(
    model: &mut BRepModel,
    edge_ids: &[EdgeId],
    context: &mut CloneContext,
) -> OperationResult<Vec<EdgeId>> {
    let mut new_ids = Vec::new();

    for &edge_id in edge_ids {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
            .clone();

        // Remap vertex IDs
        let new_start = *context.vertex_map.get(&edge.start_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("Start vertex not in clone context".to_string())
        })?;
        let new_end = *context.vertex_map.get(&edge.end_vertex).ok_or_else(|| {
            OperationError::InvalidGeometry("End vertex not in clone context".to_string())
        })?;

        // Remap curve ID
        let new_curve = *context.curve_map.get(&edge.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Curve not in clone context".to_string())
        })?;

        let new_edge = Edge::new(
            0, // Will be assigned by store
            new_start,
            new_end,
            new_curve,
            edge.orientation,
            edge.param_range,
        );

        let new_id = model.edges.add(new_edge);
        context.edge_map.insert(edge_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone loops with remapped edge references
pub fn clone_loops(
    model: &mut BRepModel,
    loop_ids: &[LoopId],
    context: &mut CloneContext,
) -> OperationResult<Vec<LoopId>> {
    let mut new_ids = Vec::new();

    for &loop_id in loop_ids {
        let loop_data = model
            .loops
            .get(loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
            .clone();

        // Remap edge IDs
        let new_edges: Vec<EdgeId> = loop_data
            .edges
            .iter()
            .map(|&edge_id| {
                context.edge_map.get(&edge_id).copied().ok_or_else(|| {
                    OperationError::InvalidGeometry("Edge not in clone context".to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut new_loop = Loop::new(0, loop_data.loop_type);
        for (i, &edge_id) in new_edges.iter().enumerate() {
            new_loop.add_edge(edge_id, loop_data.orientations[i]);
        }

        let new_id = model.loops.add(new_loop);
        context.loop_map.insert(loop_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone surfaces
pub fn clone_surfaces(
    model: &mut BRepModel,
    surface_ids: &[SurfaceId],
    context: &mut CloneContext,
) -> OperationResult<Vec<SurfaceId>> {
    let mut new_ids = Vec::new();

    for &surface_id in surface_ids {
        // Use existing clone_surface method
        let new_id = model.surfaces.clone_surface(surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Failed to clone surface".to_string())
        })?;

        context.surface_map.insert(surface_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone faces with remapped surface and loop references
pub fn clone_faces(
    model: &mut BRepModel,
    face_ids: &[FaceId],
    context: &mut CloneContext,
) -> OperationResult<Vec<FaceId>> {
    let mut new_ids = Vec::new();

    for &face_id in face_ids {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
            .clone();

        // Remap surface ID
        let new_surface = *context.surface_map.get(&face.surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry("Surface not in clone context".to_string())
        })?;

        // Remap outer loop
        let new_outer_loop = *context.loop_map.get(&face.outer_loop).ok_or_else(|| {
            OperationError::InvalidGeometry("Outer loop not in clone context".to_string())
        })?;

        // Remap inner loops
        let new_inner_loops: Vec<LoopId> = face
            .inner_loops
            .iter()
            .map(|&loop_id| {
                context.loop_map.get(&loop_id).copied().ok_or_else(|| {
                    OperationError::InvalidGeometry("Inner loop not in clone context".to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut new_face = Face::new(0, new_surface, new_outer_loop, face.orientation);
        for &inner_loop in &new_inner_loops {
            new_face.add_inner_loop(inner_loop);
        }

        let new_id = model.faces.add(new_face);
        context.face_map.insert(face_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone shells with remapped face references
pub fn clone_shells(
    model: &mut BRepModel,
    shell_ids: &[ShellId],
    context: &mut CloneContext,
) -> OperationResult<Vec<ShellId>> {
    let mut new_ids = Vec::new();

    for &shell_id in shell_ids {
        let shell = model
            .shells
            .get(shell_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?
            .clone();

        // Remap face IDs
        let new_faces: Vec<FaceId> = shell
            .faces
            .iter()
            .map(|&face_id| {
                context.face_map.get(&face_id).copied().ok_or_else(|| {
                    OperationError::InvalidGeometry("Face not in clone context".to_string())
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut new_shell = Shell::new(0, shell.shell_type);
        for &face_id in &new_faces {
            new_shell.add_face(face_id);
        }

        let new_id = model.shells.add(new_shell);
        context.shell_map.insert(shell_id, new_id);
        new_ids.push(new_id);
    }

    Ok(new_ids)
}

/// Deep clone a complete solid with all its dependencies
pub fn deep_clone_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertex_offset: Option<Vector3>,
) -> OperationResult<SolidId> {
    let mut context = CloneContext::new();

    // Get the solid
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    // Collect all entities to clone
    let mut all_vertices = Vec::new();
    let mut all_curves = Vec::new();
    let mut all_edges = Vec::new();
    let mut all_loops = Vec::new();
    let mut all_surfaces = Vec::new();
    let mut all_faces = Vec::new();
    let mut all_shells = vec![solid.outer_shell];
    all_shells.extend(&solid.inner_shells);

    // Traverse the topology to collect all entities
    for &shell_id in &all_shells {
        let shell = model
            .shells
            .get(shell_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

        for &face_id in &shell.faces {
            all_faces.push(face_id);

            let face = model
                .faces
                .get(face_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

            all_surfaces.push(face.surface_id);
            all_loops.push(face.outer_loop);
            all_loops.extend(&face.inner_loops);

            for &loop_id in [face.outer_loop].iter().chain(&face.inner_loops) {
                let loop_data = model
                    .loops
                    .get(loop_id)
                    .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

                for &edge_id in &loop_data.edges {
                    all_edges.push(edge_id);

                    let edge = model.edges.get(edge_id).ok_or_else(|| {
                        OperationError::InvalidGeometry("Edge not found".to_string())
                    })?;

                    all_curves.push(edge.curve_id);
                    all_vertices.push(edge.start_vertex);
                    all_vertices.push(edge.end_vertex);
                }
            }
        }
    }

    // Remove duplicates
    all_vertices.sort_unstable();
    all_vertices.dedup();
    all_curves.sort_unstable();
    all_curves.dedup();
    all_edges.sort_unstable();
    all_edges.dedup();
    all_loops.sort_unstable();
    all_loops.dedup();
    all_surfaces.sort_unstable();
    all_surfaces.dedup();
    all_faces.sort_unstable();
    all_faces.dedup();

    // Clone in dependency order
    clone_vertices(model, &all_vertices, vertex_offset, &mut context)?;
    clone_curves(model, &all_curves, &mut context)?;
    clone_surfaces(model, &all_surfaces, &mut context)?;
    clone_edges(model, &all_edges, &mut context)?;
    clone_loops(model, &all_loops, &mut context)?;
    clone_faces(model, &all_faces, &mut context)?;
    let new_shells = clone_shells(model, &all_shells, &mut context)?;

    // Create new solid
    let new_outer_shell = new_shells[0];
    let new_inner_shells = &new_shells[1..];

    let mut new_solid = Solid::new(0, new_outer_shell);
    for &inner_shell in new_inner_shells {
        new_solid.add_inner_shell(inner_shell);
    }

    let new_solid_id = model.solids.add(new_solid);
    Ok(new_solid_id)
}

/// Deep clone selected faces from a solid (for partial cloning)
pub fn deep_clone_faces(
    model: &mut BRepModel,
    face_ids: &[FaceId],
    exclude_faces: &[FaceId],
) -> OperationResult<Vec<FaceId>> {
    let mut context = CloneContext::new();

    // Collect all entities to clone
    let mut all_vertices = Vec::new();
    let mut all_curves = Vec::new();
    let mut all_edges = Vec::new();
    let mut all_loops = Vec::new();
    let mut all_surfaces = Vec::new();

    // Filter faces
    let faces_to_clone: Vec<FaceId> = face_ids
        .iter()
        .filter(|&&id| !exclude_faces.contains(&id))
        .copied()
        .collect();

    // Traverse the topology to collect all entities
    for &face_id in &faces_to_clone {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        all_surfaces.push(face.surface_id);
        all_loops.push(face.outer_loop);
        all_loops.extend(&face.inner_loops);

        for &loop_id in [face.outer_loop].iter().chain(&face.inner_loops) {
            let loop_data = model
                .loops
                .get(loop_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;

            for &edge_id in &loop_data.edges {
                all_edges.push(edge_id);

                let edge = model
                    .edges
                    .get(edge_id)
                    .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

                all_curves.push(edge.curve_id);
                all_vertices.push(edge.start_vertex);
                all_vertices.push(edge.end_vertex);
            }
        }
    }

    // Remove duplicates
    all_vertices.sort_unstable();
    all_vertices.dedup();
    all_curves.sort_unstable();
    all_curves.dedup();
    all_edges.sort_unstable();
    all_edges.dedup();
    all_loops.sort_unstable();
    all_loops.dedup();
    all_surfaces.sort_unstable();
    all_surfaces.dedup();

    // Clone in dependency order
    clone_vertices(model, &all_vertices, None, &mut context)?;
    clone_curves(model, &all_curves, &mut context)?;
    clone_surfaces(model, &all_surfaces, &mut context)?;
    clone_edges(model, &all_edges, &mut context)?;
    clone_loops(model, &all_loops, &mut context)?;
    let new_faces = clone_faces(model, &faces_to_clone, &mut context)?;

    Ok(new_faces)
}
