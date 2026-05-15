//! Sew Operations for B-Rep Models
//!
//! Stitches faces together to create watertight shells and solids
//! by matching and merging coincident edges and vertices.
//!
//! Indexed access into edge/vertex enumeration arrays is the canonical
//! idiom — all `arr[i]` sites use indices bounded by topology length.
//! Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::diagnostics::BlendFailure;
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance};
use crate::primitives::{
    edge::EdgeId,
    face::FaceId,
    shell::{Shell, ShellType},
    solid::{Solid, SolidId},
    topology_builder::BRepModel,
    validation::{validate_shell_closure, ValidationError},
    vertex::VertexId,
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

    /// Verify geometric continuity of any edge already shared (same `EdgeId`)
    /// by 2+ input faces before mutating state. Samples 5 points along
    /// each such edge and projects onto every adjacent face's surface;
    /// rejects with [`OperationError::BlendFailed`] carrying a
    /// [`BlendFailure::SewGapTooLarge`] payload if any sample deviates by
    /// more than the edge's per-edge tolerance.
    ///
    /// This is the F7-δ pre-sew gate. It is a no-op for the legacy
    /// "find matching edges by sampling" flow (because those edge pairs
    /// have *different* `EdgeId`s and only become shared after merge).
    /// It catches geometry-vs-topology drift in the blend-rail path
    /// where F1', F2', and B already reference the same trim/cap edges.
    pub verify_continuity: bool,

    /// Validate that every output shell is fully closed (every edge used
    /// by exactly two faces of its shell) and reject any boundary or
    /// non-manifold edge as `InvalidBRep`. Opt-in: defaults to `false`
    /// so callers that intentionally produce open shells (e.g. partial
    /// patches) keep working.
    ///
    /// This is the F7-δ post-sew gate. Enable it on call sites where
    /// the caller knows the sew should produce closed topology — e.g.
    /// fillet/chamfer end-cap stitching, where any leftover boundary
    /// edge is a topology bug rather than a deliberate open boundary.
    /// Delegates to `primitives::validation::validate_shell_closure`.
    pub verify_closed: bool,
}

impl Default for SewOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            sew_tolerance: 1e-6,
            create_solid: true,
            merge_all: true,
            non_manifold_mode: NonManifoldMode::Reject,
            verify_continuity: true,
            verify_closed: false,
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

    // F7-δ: pre-sew geometric-continuity gate. Validates that any edge
    // already shared (by `EdgeId`) between 2+ input faces actually lies
    // on every adjacent surface within the edge's per-edge tolerance.
    // Catches the case where blend-rail topology surgery wired the
    // right `EdgeId` into multiple loops but the underlying curve does
    // not lie on one of the adjacent surfaces.
    if options.verify_continuity {
        verify_edge_geometric_continuity(model, &faces)?;
    }

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
        let tol1 = vertex.tolerance;

        for &other_id in &all_vertices {
            if processed.contains(&other_id) {
                continue;
            }

            let other = model
                .vertices
                .get(other_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;
            let pos2 = Point3::from(other.position);

            // Effective coincidence ball is the loosest of the two
            // per-vertex tolerances and the sew floor.
            let effective_tol = tol1.max(other.tolerance).max(tolerance);

            if pos1.distance(&pos2) <= effective_tol {
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

/// Check if two edges match within tolerance.
///
/// The effective gap tolerance is the maximum of the two per-edge
/// tolerances and the operation's `sew_tolerance` floor (Parasolid's
/// union-of-spheres convention: a sample gap is "coincident" iff it is
/// no larger than the loosest of the participating tolerances).
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

    let effective_tol = edge1.tolerance.max(edge2.tolerance).max(tolerance);

    // Check if edges share same geometry (opposite orientation)
    // Sample points along edges and compare
    let num_samples = 5;

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let p1 = edge1.evaluate(t, &model.curves)?;

        // Check both orientations
        let p2_forward = edge2.evaluate(t, &model.curves)?;
        let p2_reverse = edge2.evaluate(1.0 - t, &model.curves)?;

        let matches_forward = p1.distance(&p2_forward) <= effective_tol;
        let matches_reverse = p1.distance(&p2_reverse) <= effective_tol;

        if !matches_forward && !matches_reverse {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Merge vertex groups into single vertices
fn merge_vertex_groups(
    _model: &mut BRepModel,
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
    _model: &mut BRepModel,
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
            let _forward = loop_data.orientations[i];
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

    // F7-δ: opt-in post-sew closure validation per shell. Callers
    // expecting a closed result (fillet/chamfer end-cap stitching)
    // enable `verify_closed`; legacy callers that sew open patches
    // keep working without surfacing boundary-edge errors. Returns
    // the first connectivity error verbatim — its message already
    // pinpoints the offending edge and shell.
    if options.verify_closed {
        for &shell_id in shells {
            let errors = validate_shell_closure(model, shell_id);
            if let Some(err) = errors.into_iter().next() {
                return Err(connectivity_error_to_op_error(err));
            }
        }
    }

    Ok(())
}

/// Translate a `ValidationError::ConnectivityError` from
/// `validate_shell_closure` into an `OperationError` suitable for the
/// sew entry-point surface. Other variants degrade to `InvalidBRep`
/// with the debug formatting preserved.
fn connectivity_error_to_op_error(err: ValidationError) -> OperationError {
    match err {
        ValidationError::ConnectivityError { message, .. } => OperationError::InvalidBRep(message),
        other => OperationError::InvalidBRep(format!("{:?}", other)),
    }
}

/// F7-δ pre-sew gate: verify that every edge already shared (same
/// `EdgeId`) by 2 or more of `faces` lies on every adjacent surface
/// within the edge's per-edge tolerance.
///
/// Sampling: 5 evaluation points at `t ∈ {0, 0.25, 0.5, 0.75, 1.0}`
/// along each shared edge curve. For each sample we call
/// `Surface::closest_point` to find the nearest UV on each adjacent
/// face, evaluate the surface there, and compare the resulting point
/// against the sample. The maximum deviation across all samples and
/// all adjacent faces must be `≤ edge.tolerance` (the per-edge value
/// landed in F7-α; falls back to `1e-6` as the probe tolerance floor
/// for the closest-point solver).
///
/// On failure returns `OperationError::InvalidGeometry` with a
/// message including the edge id, the offending face id, the sample
/// parameter, the deviation, and the tolerance used. The model is
/// not mutated by this function.
///
/// This is a no-op when no edge is shared by 2+ input faces.
pub fn verify_edge_geometric_continuity(
    model: &BRepModel,
    faces: &[FaceId],
) -> OperationResult<()> {
    // Build edge -> faces using this edge.
    let mut edge_to_faces: HashMap<EdgeId, Vec<FaceId>> = HashMap::new();
    for &face_id in faces {
        let edges = get_face_edges(model, face_id)?;
        for edge_id in edges {
            edge_to_faces.entry(edge_id).or_default().push(face_id);
        }
    }

    for (edge_id, adjacent_faces) in edge_to_faces {
        if adjacent_faces.len() < 2 {
            continue;
        }
        let edge = model.edges.get(edge_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!("Edge {} not found", edge_id))
        })?;
        let edge_tol = edge.tolerance;
        // Probe tolerance is the closest-point solver's convergence
        // floor; not the same as the acceptance threshold. Use the
        // looser of edge_tol and 1e-6 to keep the solver well-posed
        // when an edge advertises a very tight tolerance.
        let probe_tol = Tolerance::from_distance(edge_tol.max(1e-6));

        for i in 0..=4u32 {
            let t = f64::from(i) / 4.0;
            let sample = edge.evaluate(t, &model.curves).map_err(|e| {
                OperationError::InvalidGeometry(format!(
                    "Cannot evaluate edge {} at t={}: {:?}",
                    edge_id, t, e
                ))
            })?;
            for &face_id in &adjacent_faces {
                let face = model.faces.get(face_id).ok_or_else(|| {
                    OperationError::InvalidGeometry(format!("Face {} not found", face_id))
                })?;
                let surface = model.surfaces.get(face.surface_id).ok_or_else(|| {
                    OperationError::InvalidGeometry(format!(
                        "Surface {} not found",
                        face.surface_id
                    ))
                })?;
                let (u, v) = surface.closest_point(&sample, probe_tol).map_err(|e| {
                    OperationError::InvalidGeometry(format!(
                        "closest_point failed on face {} for edge {}: {:?}",
                        face_id, edge_id, e
                    ))
                })?;
                let proj = surface.point_at(u, v).map_err(|e| {
                    OperationError::InvalidGeometry(format!(
                        "point_at failed on face {} for edge {}: {:?}",
                        face_id, edge_id, e
                    ))
                })?;
                let dev = sample.distance(&proj);
                if dev > edge_tol {
                    return Err(OperationError::BlendFailed(Box::new(
                        BlendFailure::SewGapTooLarge {
                            edge: edge_id,
                            gap: dev,
                            tolerance: edge_tol,
                        },
                    )));
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::{box_primitive::BoxPrimitive, topology_builder::BRepModel};

    fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
        BoxPrimitive::builder()
            .dimensions(w, h, d)
            .expect("dimensions")
            .build(model)
            .expect("build box")
    }

    /// Box has six manifold faces — every shared edge is genuinely
    /// coincident on both adjacent surfaces. Continuity gate must
    /// accept it cleanly.
    #[test]
    fn verify_continuity_accepts_box_faces() {
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 2.0, 3.0, 4.0);
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        let faces: Vec<FaceId> = shell.faces.clone();
        verify_edge_geometric_continuity(&model, &faces)
            .expect("box faces share consistent edges");
    }

    /// Single face has no shared edges with itself — the function
    /// must early-return Ok without sampling.
    #[test]
    fn verify_continuity_single_face_is_noop() {
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 1.0, 1.0, 1.0);
        let solid = model.solids.get(solid_id).expect("solid");
        let shell = model.shells.get(solid.outer_shell).expect("shell");
        let faces = vec![shell.faces[0]];
        verify_edge_geometric_continuity(&model, &faces).expect("single face is a no-op");
    }

    /// Rewiring a face's `surface_id` to point at a parallel surface
    /// offset by 100 units makes every edge of that face fail the
    /// continuity gate: closest_point still converges, but the
    /// projection distance is the 100-unit plane separation.
    #[test]
    fn verify_continuity_rejects_off_surface_face() {
        use crate::math::{Point3, Vector3};
        use crate::primitives::surface::Plane;
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 2.0, 2.0, 2.0);
        let shell_id = model.solids.get(solid_id).expect("solid").outer_shell;
        let faces: Vec<FaceId> = model
            .shells
            .get(shell_id)
            .expect("shell")
            .faces
            .clone();

        // Inject a plane 100 units away from the origin and rewire
        // the first face to use it. The face's loop edges still live
        // on the original box geometry, so projecting them onto the
        // new surface must yield a ~100-unit gap.
        let off_plane = Plane::new(
            Point3::new(0.0, 0.0, 100.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .expect("plane");
        let new_surface_id = model.surfaces.add(Box::new(off_plane));
        let probe_face = faces[0];
        if let Some(face) = model.faces.get_mut(probe_face) {
            face.surface_id = new_surface_id;
        }

        match verify_edge_geometric_continuity(&model, &faces) {
            Err(OperationError::BlendFailed(failure)) => match *failure {
                BlendFailure::SewGapTooLarge {
                    edge: _,
                    gap,
                    tolerance,
                } => {
                    assert!(
                        gap > tolerance,
                        "gap {} should exceed tolerance {}",
                        gap,
                        tolerance
                    );
                    assert!(
                        gap > 50.0,
                        "100-unit plane offset must surface as a large gap, got {}",
                        gap
                    );
                }
                other => panic!("expected SewGapTooLarge, got {:?}", other),
            },
            other => panic!("expected BlendFailed(SewGapTooLarge), got {:?}", other),
        }
    }

    /// Shell-closure validation should accept a closed box shell.
    #[test]
    fn shell_closure_accepts_box() {
        use crate::primitives::validation::validate_shell_closure;
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 1.0, 1.0, 1.0);
        let shell_id = model.solids.get(solid_id).expect("solid").outer_shell;
        assert!(
            validate_shell_closure(&model, shell_id).is_empty(),
            "closed box shell must validate clean"
        );
    }

    /// Removing a face from a closed shell must produce one boundary-
    /// edge report per edge of the removed face (4 for a box face).
    #[test]
    fn shell_closure_detects_boundary_after_face_removal() {
        use crate::primitives::validation::validate_shell_closure;
        let mut model = BRepModel::new();
        let solid_id = make_box(&mut model, 1.0, 1.0, 1.0);
        let shell_id = model.solids.get(solid_id).expect("solid").outer_shell;
        let removed_face = {
            let shell = model.shells.get(shell_id).expect("shell");
            shell.faces[0]
        };
        // Tear off one face to produce boundary edges.
        if let Some(shell) = model.shells.get_mut(shell_id) {
            shell.faces.retain(|&f| f != removed_face);
        }
        let errors = validate_shell_closure(&model, shell_id);
        assert_eq!(
            errors.len(),
            4,
            "removing one box face leaves 4 boundary edges; got {:?}",
            errors
        );
        for err in &errors {
            match err {
                ValidationError::ConnectivityError { message, .. } => {
                    assert!(
                        message.starts_with("Boundary edge "),
                        "unexpected message: {}",
                        message
                    );
                }
                other => panic!("unexpected variant: {:?}", other),
            }
        }
    }

    /// Unknown shell id returns a single descriptive error.
    #[test]
    fn shell_closure_reports_unknown_shell() {
        use crate::primitives::validation::validate_shell_closure;
        let model = BRepModel::new();
        let errors = validate_shell_closure(&model, 9999);
        assert_eq!(errors.len(), 1);
        match &errors[0] {
            ValidationError::ConnectivityError { message, .. } => {
                assert!(message.contains("Shell 9999 not found"), "{}", message);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
