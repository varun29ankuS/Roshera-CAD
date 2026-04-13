//! Sew Operations for B-Rep Models
//!
//! Stitches faces together to create watertight shells and solids
//! by matching and merging coincident edges and vertices.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::Loop,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    topology_builder::BRepModel,
    vertex::{Vertex, VertexId},
};
use std::collections::{HashMap, HashSet};

/// Options for sew operations
#[derive(Debug, Clone)]
pub struct SewOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Tolerance for matching edges/vertices
    pub sew_tolerance: f64,

    /// Whether to create solid if closed
    pub create_solid: bool,

    /// Whether to merge all coincident entities
    pub merge_all: bool,

    /// How to handle non-manifold results
    pub non_manifold_mode: NonManifoldMode,
}

impl Default for SewOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            sew_tolerance: 1e-6,
            create_solid: true,
            merge_all: true,
            non_manifold_mode: NonManifoldMode::Reject,
        }
    }
}

/// How to handle non-manifold geometry
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NonManifoldMode {
    /// Reject non-manifold results
    Reject,
    /// Allow non-manifold edges (3+ faces)
    AllowEdges,
    /// Allow non-manifold vertices
    AllowVertices,
    /// Allow all non-manifold geometry
    AllowAll,
}

/// Result of sewing operation
#[derive(Debug)]
pub struct SewResult {
    /// Created or modified shells
    pub shells: Vec<u32>,
    /// Created solids (if any)
    pub solids: Vec<SolidId>,
    /// Edges that were sewn
    pub sewn_edges: Vec<(EdgeId, EdgeId)>,
    /// Vertices that were merged
    pub merged_vertices: Vec<(VertexId, VertexId)>,
    /// Free edges that couldn't be sewn
    pub free_edges: Vec<EdgeId>,
}

/// Sew faces together into shells
pub fn sew_faces(
    model: &mut BRepModel,
    faces: Vec<FaceId>,
    options: SewOptions,
) -> OperationResult<SewResult> {
    // Validate inputs
    validate_sew_inputs(model, &faces, &options)?;

    // Find matching edges and vertices
    let edge_pairs = find_matching_edges(model, &faces, options.sew_tolerance)?;
    let vertex_groups = find_matching_vertices(model, &faces, options.sew_tolerance)?;

    // Merge vertices first
    let vertex_map = merge_vertex_groups(model, vertex_groups)?;

    // Update edges to use merged vertices
    update_edges_with_merged_vertices(model, &faces, &vertex_map)?;

    // Merge paired edges
    let edge_map = merge_edge_pairs(model, edge_pairs)?;

    // Update face loops to use merged edges
    update_face_loops(model, &faces, &edge_map)?;

    // Build shells from connected faces
    let shells = build_shells_from_faces(model, &faces)?;

    // Check for closed shells and create solids
    let mut solids = Vec::new();
    if options.create_solid {
        for &shell_id in &shells {
            if is_shell_closed(model, shell_id)? {
                let solid = Solid::new(0, shell_id); // ID will be assigned by store
                let solid_id = model.solids.add(solid);
                solids.push(solid_id);
            }
        }
    }

    // Find remaining free edges
    let free_edges = find_free_edges(model, &shells)?;

    // Validate result if requested
    if options.common.validate_result {
        validate_sew_result(model, &shells, &options)?;
    }

    Ok(SewResult {
        shells,
        solids,
        sewn_edges: edge_map.into_iter().collect(),
        merged_vertices: vertex_map.into_iter().collect(),
        free_edges,
    })
}

/// Find matching edges within tolerance
fn find_matching_edges(
    model: &BRepModel,
    faces: &[FaceId],
    tolerance: f64,
) -> OperationResult<Vec<(EdgeId, EdgeId)>> {
    let mut edge_pairs = Vec::new();
    let mut processed = HashSet::new();

    // Get all edges from faces
    let all_edges = get_all_edges_from_faces(model, faces)?;

    // Compare each pair of edges
    for i in 0..all_edges.len() {
        if processed.contains(&all_edges[i]) {
            continue;
        }

        for j in (i + 1)..all_edges.len() {
            if processed.contains(&all_edges[j]) {
                continue;
            }

            if edges_match(model, all_edges[i], all_edges[j], tolerance)? {
                edge_pairs.push((all_edges[i], all_edges[j]));
                processed.insert(all_edges[i]);
                processed.insert(all_edges[j]);
                break;
            }
        }
    }

    Ok(edge_pairs)
}

/// Find matching vertices within tolerance
fn find_matching_vertices(
    model: &BRepModel,
    faces: &[FaceId],
    tolerance: f64,
) -> OperationResult<Vec<Vec<VertexId>>> {
    let mut vertex_groups = Vec::new();
    let mut processed = HashSet::new();

    // Get all vertices from faces
    let all_vertices = get_all_vertices_from_faces(model, faces)?;

    // Group vertices within tolerance
    for &vertex_id in &all_vertices {
        if processed.contains(&vertex_id) {
            continue;
        }

        let mut group = vec![vertex_id];
        processed.insert(vertex_id);

        let vertex = model
            .vertices
            .get(vertex_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
        let pos1 = Point3::from(vertex.position);

        for &other_id in &all_vertices {
            if processed.contains(&other_id) {
                continue;
            }

            let other = model
                .vertices
                .get(other_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
            let pos2 = Point3::from(other.position);

            if pos1.distance(&pos2) <= tolerance {
                group.push(other_id);
                processed.insert(other_id);
            }
        }

        if group.len() > 1 {
            vertex_groups.push(group);
        }
    }

    Ok(vertex_groups)
}

/// Check if two edges match within tolerance
fn edges_match(
    model: &BRepModel,
    edge1_id: EdgeId,
    edge2_id: EdgeId,
    tolerance: f64,
) -> OperationResult<bool> {
    let edge1 = model
        .edges
        .get(edge1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
    let edge2 = model
        .edges
        .get(edge2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Check if edges share same geometry (opposite orientation)
    // Sample points along edges and compare
    let num_samples = 5;

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let p1 = edge1.evaluate(t, &model.curves)?;

        // Check both orientations
        let p2_forward = edge2.evaluate(t, &model.curves)?;
        let p2_reverse = edge2.evaluate(1.0 - t, &model.curves)?;

        let matches_forward = p1.distance(&p2_forward) <= tolerance;
        let matches_reverse = p1.distance(&p2_reverse) <= tolerance;

        if !matches_forward && !matches_reverse {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Merge vertex groups into single vertices
fn merge_vertex_groups(
    model: &mut BRepModel,
    vertex_groups: Vec<Vec<VertexId>>,
) -> OperationResult<HashMap<VertexId, VertexId>> {
    let mut vertex_map = HashMap::new();

    for group in vertex_groups {
        if group.len() < 2 {
            continue;
        }

        // Use first vertex as the representative
        let target_vertex = group[0];

        // Map all others to the target
        for &vertex_id in &group[1..] {
            vertex_map.insert(vertex_id, target_vertex);
        }
    }

    Ok(vertex_map)
}

/// Merge paired edges
fn merge_edge_pairs(
    model: &mut BRepModel,
    edge_pairs: Vec<(EdgeId, EdgeId)>,
) -> OperationResult<HashMap<EdgeId, EdgeId>> {
    let mut edge_map = HashMap::new();

    for (edge1_id, edge2_id) in edge_pairs {
        // Keep first edge, map second to first
        edge_map.insert(edge2_id, edge1_id);
    }

    Ok(edge_map)
}

/// Update edges to use merged vertices
fn update_edges_with_merged_vertices(
    model: &mut BRepModel,
    faces: &[FaceId],
    vertex_map: &HashMap<VertexId, VertexId>,
) -> OperationResult<()> {
    let all_edges = get_all_edges_from_faces(model, faces)?;

    for edge_id in all_edges {
        if let Some(edge) = model.edges.get_mut(edge_id) {
            // Update start vertex if mapped
            if let Some(&new_vertex) = vertex_map.get(&edge.start_vertex) {
                edge.start_vertex = new_vertex;
            }
            // Update end vertex if mapped
            if let Some(&new_vertex) = vertex_map.get(&edge.end_vertex) {
                edge.end_vertex = new_vertex;
            }
        }
    }

    Ok(())
}

/// Update face loops to use merged edges
fn update_face_loops(
    model: &mut BRepModel,
    faces: &[FaceId],
    edge_map: &HashMap<EdgeId, EdgeId>,
) -> OperationResult<()> {
    for &face_id in faces {
        // Collect loop ids to avoid borrowing issues
        let (outer_loop, inner_loops) = {
            let face = model
                .faces
                .get(face_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
            (face.outer_loop, face.inner_loops.clone())
        };

        // Update outer loop
        update_loop_edges(model, outer_loop, edge_map)?;

        // Update inner loops
        for inner_loop_id in inner_loops {
            update_loop_edges(model, inner_loop_id, edge_map)?;
        }
    }

    Ok(())
}

/// Update loop edges based on edge map
fn update_loop_edges(
    model: &mut BRepModel,
    loop_id: u32,
    edge_map: &HashMap<EdgeId, EdgeId>,
) -> OperationResult<()> {
    if let Some(loop_data) = model.loops.get_mut(loop_id) {
        let mut new_edges = Vec::new();

        for (i, &edge_id) in loop_data.edges.iter().enumerate() {
            let forward = loop_data.orientations[i];
            let mapped_edge = edge_map.get(&edge_id).copied().unwrap_or(edge_id);
            new_edges.push(mapped_edge);
        }

        loop_data.edges = new_edges;
    }

    Ok(())
}

/// Build shells from connected faces
fn build_shells_from_faces(model: &mut BRepModel, faces: &[FaceId]) -> OperationResult<Vec<u32>> {
    let mut shells = Vec::new();
    let mut processed = HashSet::new();

    for &face_id in faces {
        if processed.contains(&face_id) {
            continue;
        }

        // Find all connected faces
        let connected_faces = find_connected_faces(model, face_id, faces)?;
        for &connected_face in &connected_faces {
            processed.insert(connected_face);
        }

        // Create shell from connected faces
        let mut shell = Shell::new(
            0,               // ID will be assigned by store
            ShellType::Open, // Will check later
        );

        for &connected_face in &connected_faces {
            shell.add_face(connected_face);
        }

        let shell_id = model.shells.add(shell);
        shells.push(shell_id);
    }

    Ok(shells)
}

/// Find all faces connected to a given face
fn find_connected_faces(
    model: &BRepModel,
    start_face: FaceId,
    candidate_faces: &[FaceId],
) -> OperationResult<Vec<FaceId>> {
    let mut connected = vec![start_face];
    let mut to_process = vec![start_face];
    let mut processed = HashSet::new();

    while let Some(face_id) = to_process.pop() {
        if processed.contains(&face_id) {
            continue;
        }
        processed.insert(face_id);

        // Get edges of current face
        let face_edges = get_face_edges(model, face_id)?;

        // Find faces that share edges
        for &candidate in candidate_faces {
            if processed.contains(&candidate) || candidate == face_id {
                continue;
            }

            let candidate_edges = get_face_edges(model, candidate)?;

            // Check if faces share any edge
            if face_edges.iter().any(|e| candidate_edges.contains(e)) {
                connected.push(candidate);
                to_process.push(candidate);
            }
        }
    }

    Ok(connected)
}

/// Get all edges from a face
fn get_face_edges(model: &BRepModel, face_id: FaceId) -> OperationResult<Vec<EdgeId>> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let mut edges = Vec::new();

    // Get outer loop edges
    if let Some(outer_loop) = model.loops.get(face.outer_loop) {
        for &edge_id in &outer_loop.edges {
            edges.push(edge_id);
        }
    }

    // Get inner loop edges
    for &inner_loop_id in &face.inner_loops {
        if let Some(inner_loop) = model.loops.get(inner_loop_id) {
            for &edge_id in &inner_loop.edges {
                edges.push(edge_id);
            }
        }
    }

    Ok(edges)
}

/// Get all edges from multiple faces
fn get_all_edges_from_faces(model: &BRepModel, faces: &[FaceId]) -> OperationResult<Vec<EdgeId>> {
    let mut all_edges = Vec::new();

    for &face_id in faces {
        let edges = get_face_edges(model, face_id)?;
        all_edges.extend(edges);
    }

    // Remove duplicates
    all_edges.sort_unstable();
    all_edges.dedup();

    Ok(all_edges)
}

/// Get all vertices from multiple faces
fn get_all_vertices_from_faces(
    model: &BRepModel,
    faces: &[FaceId],
) -> OperationResult<Vec<VertexId>> {
    let mut all_vertices = HashSet::new();

    for &face_id in faces {
        let edges = get_face_edges(model, face_id)?;

        for edge_id in edges {
            if let Some(edge) = model.edges.get(edge_id) {
                all_vertices.insert(edge.start_vertex);
                all_vertices.insert(edge.end_vertex);
            }
        }
    }

    Ok(all_vertices.into_iter().collect())
}

/// Check if shell is closed
fn is_shell_closed(model: &BRepModel, shell_id: u32) -> OperationResult<bool> {
    let shell = model
        .shells
        .get(shell_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    // Count edge usage - in closed shell, each edge used exactly twice
    let mut edge_usage = HashMap::new();

    for &face_id in &shell.faces {
        let edges = get_face_edges(model, face_id)?;
        for edge_id in edges {
            *edge_usage.entry(edge_id).or_insert(0) += 1;
        }
    }

    // Check all edges are used exactly twice
    for (_, count) in edge_usage {
        if count != 2 {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Find free edges in shells
fn find_free_edges(model: &BRepModel, shells: &[u32]) -> OperationResult<Vec<EdgeId>> {
    let mut edge_usage = HashMap::new();

    // Count edge usage across all shells
    for &shell_id in shells {
        if let Some(shell) = model.shells.get(shell_id) {
            for &face_id in &shell.faces {
                let edges = get_face_edges(model, face_id)?;
                for edge_id in edges {
                    *edge_usage.entry(edge_id).or_insert(0) += 1;
                }
            }
        }
    }

    // Free edges are used only once
    let free_edges: Vec<_> = edge_usage
        .into_iter()
        .filter_map(
            |(edge_id, count)| {
                if count == 1 {
                    Some(edge_id)
                } else {
                    None
                }
            },
        )
        .collect();

    Ok(free_edges)
}

/// Validate sew inputs
fn validate_sew_inputs(
    model: &BRepModel,
    faces: &[FaceId],
    options: &SewOptions,
) -> OperationResult<()> {
    if faces.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "No faces to sew".to_string(),
        ));
    }

    // Check all faces exist
    for &face_id in faces {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Face not found".to_string(),
            ));
        }
    }

    if options.sew_tolerance <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Sew tolerance must be positive".to_string(),
        ));
    }

    Ok(())
}

/// Validate sew result
fn validate_sew_result(
    model: &BRepModel,
    shells: &[u32],
    options: &SewOptions,
) -> OperationResult<()> {
    // Check for non-manifold conditions if not allowed
    if options.non_manifold_mode == NonManifoldMode::Reject {
        for &shell_id in shells {
            if has_non_manifold_edges(model, shell_id)? {
                return Err(OperationError::InvalidBRep(
                    "Non-manifold edges detected".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Check if shell has non-manifold edges
fn has_non_manifold_edges(model: &BRepModel, shell_id: u32) -> OperationResult<bool> {
    let shell = model
        .shells
        .get(shell_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut edge_usage = HashMap::new();

    for &face_id in &shell.faces {
        let edges = get_face_edges(model, face_id)?;
        for edge_id in edges {
            *edge_usage.entry(edge_id).or_insert(0) += 1;
        }
    }

    // Non-manifold if any edge used more than twice
    for (_, count) in edge_usage {
        if count > 2 {
            return Ok(true);
        }
    }

    Ok(false)
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_sew_validation() {
//         // Test parameter validation
//     }
// }
