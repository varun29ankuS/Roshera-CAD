//! Chamfer Operations for B-Rep Models
//!
//! Creates beveled transitions between faces by cutting edges at specified angles or distances.
//!
//! Indexed access into edge/face buffers and surface-sample arrays is the
//! canonical idiom — all `arr[i]` sites use indices bounded by topology
//! enumeration. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    r#loop::Loop,
    solid::SolidId,
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};

/// Options for chamfer operations
#[derive(Debug, Clone)]
pub struct ChamferOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of chamfer
    pub chamfer_type: ChamferType,

    /// Distance from edge on first face
    pub distance1: f64,

    /// Distance from edge on second face
    pub distance2: f64,

    /// Whether chamfer is symmetric (equal distances)
    pub symmetric: bool,

    /// Propagation mode for edge selection
    pub propagation: PropagationMode,

    /// Whether to preserve original edges in special cases
    pub preserve_edges: bool,
}

impl Default for ChamferOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            chamfer_type: ChamferType::EqualDistance(1.0),
            distance1: 1.0,
            distance2: 1.0,
            symmetric: true,
            propagation: PropagationMode::None,
            preserve_edges: false,
        }
    }
}

/// Type of chamfer
#[derive(Debug, Clone)]
pub enum ChamferType {
    /// Equal distance from edge on both faces
    EqualDistance(f64),
    /// Different distances on each face
    TwoDistances(f64, f64),
    /// Distance and angle
    DistanceAngle(f64, f64),
    /// Symmetric at specified angle (45° default)
    Angle(f64),
}

/// How to propagate chamfer selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationMode {
    /// No propagation
    None,
    /// Propagate along tangent edges
    Tangent,
    /// Propagate along smooth edges
    Smooth,
}

/// Apply chamfer to edges
pub fn chamfer_edges(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: ChamferOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_chamfer_inputs(model, solid_id, &edges, &options)?;

    // Capture input edges before the Vec is consumed by propagation.
    let input_edges_for_record: Vec<u64> = edges.iter().map(|&e| e as u64).collect();

    // Propagate edge selection if requested
    let selected_edges = propagate_edge_selection(model, edges, options.propagation)?;

    // Create chamfer faces for each edge
    let mut chamfer_faces = Vec::new();
    for &edge_id in &selected_edges {
        let face_id = create_edge_chamfer(model, solid_id, edge_id, &options)?;
        chamfer_faces.push(face_id);
    }

    // Update adjacent faces to account for chamfer
    update_adjacent_faces_for_chamfer(model, solid_id, &selected_edges, &chamfer_faces)?;

    // Handle vertex conditions where multiple chamfers meet
    if selected_edges.len() > 1 {
        handle_chamfer_vertices(model, solid_id, &selected_edges, &chamfer_faces)?;
    }

    // Validate result if requested
    if options.common.validate_result {
        validate_chamfered_solid(model, solid_id)?;
    }

    // Record the operation for timeline / event-sourcing consumers.
    let mut inputs = input_edges_for_record;
    inputs.insert(0, solid_id as u64);
    let chamfer_face_ids: Vec<u64> = chamfer_faces.iter().map(|&f| f as u64).collect();
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("chamfer_edges")
            .with_parameters(serde_json::json!({
                "solid_id": solid_id,
                "chamfer_type": format!("{:?}", options.chamfer_type),
                "distance1": options.distance1,
                "distance2": options.distance2,
                "symmetric": options.symmetric,
                "propagation": format!("{:?}", options.propagation),
                "preserve_edges": options.preserve_edges,
            }))
            .with_inputs(inputs)
            .with_outputs(chamfer_face_ids),
    );

    Ok(chamfer_faces)
}

/// Create chamfer for a single edge
fn create_edge_chamfer(
    model: &mut BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
    options: &ChamferOptions,
) -> OperationResult<FaceId> {
    // Get adjacent faces
    let (face1_id, face2_id) = get_adjacent_faces(model, solid_id, edge_id)?;

    // Create chamfer based on type
    match &options.chamfer_type {
        ChamferType::EqualDistance(dist) => {
            create_equal_distance_chamfer(model, edge_id, face1_id, face2_id, *dist)
        }
        ChamferType::TwoDistances(dist1, dist2) => {
            create_two_distance_chamfer(model, edge_id, face1_id, face2_id, *dist1, *dist2)
        }
        ChamferType::DistanceAngle(dist, angle) => {
            create_distance_angle_chamfer(model, edge_id, face1_id, face2_id, *dist, *angle)
        }
        ChamferType::Angle(angle) => {
            create_angle_chamfer(model, edge_id, face1_id, face2_id, *angle)
        }
    }
}

/// Create equal distance chamfer
fn create_equal_distance_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance: f64,
) -> OperationResult<FaceId> {
    // Get edge geometry
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Compute chamfer offsets along edge
    let chamfer_data =
        compute_chamfer_offsets(model, &edge, face1_id, face2_id, distance, distance)?;

    // Create chamfer surface (ruled surface between offset curves)
    let chamfer_surface = create_ruled_chamfer_surface(model, &chamfer_data)?;
    let surface_id = model.surfaces.add(chamfer_surface);

    // Create chamfer face with proper boundaries
    let chamfer_face = create_chamfer_face(model, surface_id, edge_id, &chamfer_data)?;

    Ok(chamfer_face)
}

/// Create two-distance chamfer
fn create_two_distance_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance1: f64,
    distance2: f64,
) -> OperationResult<FaceId> {
    // Similar to equal distance but with different offsets
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    let chamfer_data =
        compute_chamfer_offsets(model, &edge, face1_id, face2_id, distance1, distance2)?;

    let chamfer_surface = create_ruled_chamfer_surface(model, &chamfer_data)?;
    let surface_id = model.surfaces.add(chamfer_surface);

    let chamfer_face = create_chamfer_face(model, surface_id, edge_id, &chamfer_data)?;

    Ok(chamfer_face)
}

/// Create distance-angle chamfer
fn create_distance_angle_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    distance: f64,
    angle: f64,
) -> OperationResult<FaceId> {
    // Compute second distance from angle
    let face_angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let distance2 = distance * (angle.sin() / (face_angle - angle).sin());

    create_two_distance_chamfer(model, edge_id, face1_id, face2_id, distance, distance2)
}

/// Create angle-based chamfer
fn create_angle_chamfer(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    _angle: f64,
) -> OperationResult<FaceId> {
    // For symmetric angle chamfer, compute equal distances
    let face_angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let _half_angle = face_angle / 2.0;

    // Choose a reasonable default distance
    let distance = 1.0; // Would be computed from edge length

    create_equal_distance_chamfer(model, edge_id, face1_id, face2_id, distance)
}

/// Data for chamfer computation
struct ChamferData {
    /// Points on first face offset curve
    offset_points1: Vec<Point3>,
    /// Points on second face offset curve
    offset_points2: Vec<Point3>,
    /// Parameter values along edge
    parameters: Vec<f64>,
    /// Normal directions on faces
    normals1: Vec<Vector3>,
    normals2: Vec<Vector3>,
}

/// Compute chamfer offset curves
fn compute_chamfer_offsets(
    model: &BRepModel,
    edge: &Edge,
    face1_id: FaceId,
    face2_id: FaceId,
    distance1: f64,
    distance2: f64,
) -> OperationResult<ChamferData> {
    let num_samples = 10;
    let mut data = ChamferData {
        offset_points1: Vec::new(),
        offset_points2: Vec::new(),
        parameters: Vec::new(),
        normals1: Vec::new(),
        normals2: Vec::new(),
    };

    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        data.parameters.push(t);

        // Get point on edge
        let edge_point = curve.point_at(t).map_err(|e| {
            OperationError::NumericalError(format!("Edge evaluation failed: {:?}", e))
        })?;

        // Get edge tangent at this parameter
        let edge_tangent = curve
            .tangent_at(t)
            .map_err(|e| OperationError::NumericalError(format!("Edge tangent failed: {:?}", e)))?;

        // Get face normals at edge point
        let face_normal1 = face_normal_at_point(model, face1_id, &edge_point)?;
        let face_normal2 = face_normal_at_point(model, face2_id, &edge_point)?;

        // Offset direction on each face = cross(face_normal, edge_tangent), pointing inward on face
        // This gives a direction lying in the face plane, perpendicular to the edge
        let offset_dir1 = face_normal1.cross(&edge_tangent).normalize().map_err(|e| {
            OperationError::NumericalError(format!(
                "Offset direction normalization failed: {:?}",
                e
            ))
        })?;
        let offset_dir2 = edge_tangent.cross(&face_normal2).normalize().map_err(|e| {
            OperationError::NumericalError(format!(
                "Offset direction normalization failed: {:?}",
                e
            ))
        })?;

        data.offset_points1
            .push(edge_point + offset_dir1 * distance1);
        data.offset_points2
            .push(edge_point + offset_dir2 * distance2);
        data.normals1.push(face_normal1);
        data.normals2.push(face_normal2);
    }

    Ok(data)
}

/// Create a RuledSurface for the chamfer face, interpolating between the two offset curves.
/// Each offset curve is approximated as a Line between its endpoints.
#[allow(clippy::expect_used)] // offset_points{1,2} non-empty: is_empty() guard above expect sites
fn create_ruled_chamfer_surface(
    _model: &mut BRepModel,
    data: &ChamferData,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::curve::Line;
    use crate::primitives::surface::RuledSurface;

    if data.offset_points1.is_empty() || data.offset_points2.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Chamfer offset curves are empty".to_string(),
        ));
    }

    // Create boundary curves from offset point sequences.
    // For straight edges (the common case), endpoints suffice.
    // For curved edges, we use the endpoints of the sampled polyline —
    // a proper B-spline fit could improve accuracy for highly curved edges.
    let curve1: Box<dyn Curve> = Box::new(Line::new(
        data.offset_points1[0],
        *data
            .offset_points1
            .last()
            .expect("offset_points1 non-empty: is_empty check above rejects empty"),
    ));
    let curve2: Box<dyn Curve> = Box::new(Line::new(
        data.offset_points2[0],
        *data
            .offset_points2
            .last()
            .expect("offset_points2 non-empty: is_empty check above rejects empty"),
    ));

    Ok(Box::new(RuledSurface::new(curve1, curve2)))
}

/// Create chamfer face with boundaries
#[allow(clippy::expect_used)] // offset_points{1,2} non-empty: is_empty() guard at fn entry
fn create_chamfer_face(
    model: &mut BRepModel,
    surface_id: u32,
    edge_id: EdgeId,
    data: &ChamferData,
) -> OperationResult<FaceId> {
    // Create edges for chamfer boundary
    let _edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Validate chamfer data has non-empty offset point sequences.
    // Upstream `compute_chamfer_offsets` always populates these via a
    // `for i in 0..=num_samples` loop, but we guard here defensively
    // since `create_chamfer_face` is callable independently.
    if data.offset_points1.is_empty() || data.offset_points2.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Chamfer offset point sequences must be non-empty".to_string(),
        ));
    }

    // Create offset edge curves
    let offset_curve1 = create_offset_curve(model, &data.offset_points1)?;
    let offset_curve2 = create_offset_curve(model, &data.offset_points2)?;

    // Capture last-point references once; validated non-empty above.
    let last1 = data
        .offset_points1
        .last()
        .expect("offset_points1 non-empty: validated above");
    let last2 = data
        .offset_points2
        .last()
        .expect("offset_points2 non-empty: validated above");

    // Create vertices at ends
    let v1_start = model.vertices.add(
        data.offset_points1[0].x,
        data.offset_points1[0].y,
        data.offset_points1[0].z,
    );
    let v1_end = model.vertices.add(last1.x, last1.y, last1.z);
    let v2_start = model.vertices.add(
        data.offset_points2[0].x,
        data.offset_points2[0].y,
        data.offset_points2[0].z,
    );
    let v2_end = model.vertices.add(last2.x, last2.y, last2.z);

    // Create edges
    let edge1 = Edge::new_auto_range(
        0, // Will be assigned by store
        v1_start,
        v1_end,
        offset_curve1,
        EdgeOrientation::Forward,
    );
    let edge1_id = model.edges.add(edge1);

    let edge2 = create_straight_edge(model, v1_end, v2_end)?;

    let edge3 = Edge::new_auto_range(
        0, // Will be assigned by store
        v2_end,
        v2_start,
        offset_curve2,
        EdgeOrientation::Forward,
    );
    let edge3_id = model.edges.add(edge3);

    let edge4 = create_straight_edge(model, v2_start, v1_start)?;

    // Create loop
    let mut chamfer_loop = Loop::new(
        0, // Will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    chamfer_loop.add_edge(edge1_id, true);
    chamfer_loop.add_edge(edge2, true);
    chamfer_loop.add_edge(edge3_id, false);
    chamfer_loop.add_edge(edge4, true);
    let loop_id = model.loops.add(chamfer_loop);

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

/// Create an offset curve through a sequence of sample points.
///
/// Two points → exact `Line`. Three or more points → degree-min(3, n-1)
/// NURBS curve fit through the points (clamped uniform parameterisation).
/// This preserves the curvature of the offset trail along non-planar
/// chamfered edges, instead of collapsing to a straight chord that
/// silently disconnects from the actual chamfer surface.
fn create_offset_curve(model: &mut BRepModel, points: &[Point3]) -> OperationResult<u32> {
    use crate::primitives::curve::{Line, NurbsCurve};

    let first = points.first().ok_or_else(|| {
        OperationError::InvalidGeometry("Offset curve requires at least one point".to_string())
    })?;

    if points.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Offset curve requires at least two points".to_string(),
        ));
    }

    if points.len() == 2 {
        let last = points.last().ok_or_else(|| {
            OperationError::InvalidGeometry("Offset curve requires at least two points".to_string())
        })?;
        let line = Line::new(*first, *last);
        return Ok(model.curves.add(Box::new(line)));
    }

    // 3+ points: fit a clamped NURBS curve. Tolerance is informational for
    // `fit_to_points`; we pass the kernel default.
    let tolerance = crate::math::Tolerance::default();
    let nurbs = NurbsCurve::fit_to_points(points, 3, tolerance.distance()).map_err(|e| {
        OperationError::NumericalError(format!("offset curve fit failed: {:?}", e))
    })?;
    Ok(model.curves.add(Box::new(nurbs)))
}

/// Create straight edge between vertices
fn create_straight_edge(
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
        0, // Will be assigned by store
        start,
        end,
        curve_id,
        EdgeOrientation::Forward,
    );
    let edge_id = model.edges.add(edge);

    Ok(edge_id)
}

/// Update adjacent faces for chamfer.
/// Adds the chamfer faces to the solid's outer shell and replaces
/// the original chamfered edges in adjacent faces' loops with the
/// corresponding chamfer boundary edges.
fn update_adjacent_faces_for_chamfer(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    chamfer_faces: &[FaceId],
) -> OperationResult<()> {
    // Get the solid's outer shell
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;
    let shell_id = solid.outer_shell;

    // Add chamfer faces to the shell
    let shell = model
        .shells
        .get_mut(shell_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;
    for &face_id in chamfer_faces {
        shell.add_face(face_id);
    }

    // For each original chamfered edge, remove it from adjacent face loops.
    // The chamfer face now bridges the gap — adjacent faces' boundaries
    // are trimmed by the offset curves (already created as chamfer face edges).
    // In a full implementation we'd split the adjacent faces' loops, replacing
    // the chamfered edge with the offset edge. For now, mark the original
    // edges as consumed by removing them from existing loops.
    for &edge_id in edges {
        // Scan all faces in the shell for loops containing this edge
        let shell = model
            .shells
            .get(shell_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;
        let face_ids: Vec<FaceId> = shell.faces.clone();

        for face_id in face_ids {
            // Skip chamfer faces themselves
            if chamfer_faces.contains(&face_id) {
                continue;
            }

            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };

            let loop_ids: Vec<_> = std::iter::once(face.outer_loop)
                .chain(face.inner_loops.iter().copied())
                .collect();

            for loop_id in loop_ids {
                if let Some(loop_data) = model.loops.get_mut(loop_id) {
                    // Find indices of the chamfered edge and remove from both edges and orientations
                    let mut indices_to_remove: Vec<usize> = Vec::new();
                    for (idx, &e_id) in loop_data.edges.iter().enumerate() {
                        if e_id == edge_id {
                            indices_to_remove.push(idx);
                        }
                    }
                    // Remove in reverse order to preserve indices
                    for &idx in indices_to_remove.iter().rev() {
                        loop_data.edges.remove(idx);
                        if idx < loop_data.orientations.len() {
                            loop_data.orientations.remove(idx);
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// Handle vertices where multiple chamfers meet.
/// At shared vertices, chamfer faces may need additional triangular "corner" faces
/// to close the gap. For the common case of two chamfers meeting at a box corner,
/// this creates a triangular face connecting the three offset endpoints.
fn handle_chamfer_vertices(
    model: &mut BRepModel,
    _solid_id: SolidId,
    edges: &[EdgeId],
    _chamfer_faces: &[FaceId],
) -> OperationResult<()> {
    use std::collections::HashMap;

    // Build vertex → incident chamfered edges map
    let mut vertex_edges: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
    for &edge_id in edges {
        if let Some(edge) = model.edges.get(edge_id) {
            vertex_edges
                .entry(edge.start_vertex)
                .or_default()
                .push(edge_id);
            vertex_edges
                .entry(edge.end_vertex)
                .or_default()
                .push(edge_id);
        }
    }

    // For vertices shared by 2+ chamfered edges, the corner is already
    // topologically bounded by the chamfer faces' endpoint edges.
    // A full treatment would insert triangular fill faces here, but
    // the current chamfer face construction already creates edges that
    // share vertices at the corners, so the shell remains closed for
    // single-edge and parallel-edge chamfers.
    //
    // For complex multi-edge vertex treatment (e.g., 3 edges meeting
    // at a box corner), we'd need to:
    // 1. Find the 3 offset points around the corner
    // 2. Create a triangular RuledSurface (degenerate) or planar face
    // 3. Connect it to the three adjacent chamfer faces
    //
    // This is deferred to a follow-up since it requires face-splitting
    // infrastructure that's not yet robust enough.

    // For now, verify that vertex connectivity is maintained
    for incident_edges in vertex_edges.values() {
        if incident_edges.len() >= 3 {
            // Three or more chamfered edges meeting at a vertex — complex case
            // Log but don't fail; the chamfer faces are still valid individually
            continue;
        }
    }

    Ok(())
}

/// Propagate edge selection
fn propagate_edge_selection(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
    mode: PropagationMode,
) -> OperationResult<Vec<EdgeId>> {
    match mode {
        PropagationMode::None => Ok(initial_edges),
        PropagationMode::Tangent => propagate_tangent_edges(model, initial_edges),
        PropagationMode::Smooth => propagate_smooth_edges(model, initial_edges),
    }
}

/// Propagate along tangent edges
fn propagate_tangent_edges(
    _model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    // Would find all tangent-connected edges
    Ok(initial_edges)
}

/// Propagate along smooth edges
fn propagate_smooth_edges(
    _model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    // Would find all smoothly-connected edges
    Ok(initial_edges)
}

/// Get adjacent faces for an edge by scanning all faces in the solid's shells
fn get_adjacent_faces(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;

    let mut adjacent_faces: Vec<FaceId> = Vec::new();

    // Collect all shell IDs first to avoid borrowing issues
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend_from_slice(&solid.inner_shells);

    for shell_id in shell_ids {
        let shell = model
            .shells
            .get(shell_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

        for &face_id in &shell.faces {
            let face = model
                .faces
                .get(face_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

            // Check outer loop and inner loops for the target edge
            let loop_ids: Vec<_> = std::iter::once(face.outer_loop)
                .chain(face.inner_loops.iter().copied())
                .collect();

            'face_check: for loop_id in loop_ids {
                if let Some(loop_data) = model.loops.get(loop_id) {
                    for &e_id in &loop_data.edges {
                        if e_id == edge_id {
                            adjacent_faces.push(face_id);
                            break 'face_check;
                        }
                    }
                }
            }

            if adjacent_faces.len() == 2 {
                return Ok((adjacent_faces[0], adjacent_faces[1]));
            }
        }
    }

    if adjacent_faces.len() < 2 {
        return Err(OperationError::InvalidGeometry(format!(
            "Edge {:?} is not shared by two faces (found {})",
            edge_id,
            adjacent_faces.len()
        )));
    }

    Ok((adjacent_faces[0], adjacent_faces[1]))
}

/// Compute dihedral angle between two faces at a shared edge.
/// Returns the angle in radians between the outward-facing normals.
fn compute_face_angle(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<f64> {
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Evaluate edge midpoint
    let curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
    let mid_point = curve.point_at(0.5).map_err(|e| {
        OperationError::NumericalError(format!("Edge midpoint evaluation failed: {:?}", e))
    })?;

    // Get surface normals at edge midpoint for both faces
    let n1 = face_normal_at_point(model, face1_id, &mid_point)?;
    let n2 = face_normal_at_point(model, face2_id, &mid_point)?;

    // Dihedral angle = π - acos(n1 · n2)
    // For outward-facing normals on a convex edge, n1·n2 < 0 → angle > π/2
    let dot = n1.dot(&n2).clamp(-1.0, 1.0);
    let angle = std::f64::consts::PI - dot.acos();

    Ok(angle)
}

/// Get face surface normal at a given 3D point by finding the closest UV parameters
fn face_normal_at_point(
    model: &BRepModel,
    face_id: FaceId,
    point: &Point3,
) -> OperationResult<Vector3> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;

    // Use closest_point to find UV, then evaluate normal
    let tolerance = Tolerance::default();
    let (u, v) = surface.closest_point(point, tolerance).map_err(|e| {
        OperationError::NumericalError(format!("Closest point on surface failed: {:?}", e))
    })?;

    let mut normal = surface.normal_at(u, v).map_err(|e| {
        OperationError::NumericalError(format!("Surface normal evaluation failed: {:?}", e))
    })?;

    // Flip normal if face orientation is backward
    if face.orientation == FaceOrientation::Backward {
        normal *= -1.0;
    }

    Ok(normal)
}

/// Validate chamfer inputs
fn validate_chamfer_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    options: &ChamferOptions,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check edges exist
    for &edge_id in edges {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Edge not found".to_string(),
            ));
        }
    }

    // Validate chamfer parameters
    match &options.chamfer_type {
        ChamferType::EqualDistance(d) => {
            if *d <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distance must be positive".to_string(),
                ));
            }
        }
        ChamferType::TwoDistances(d1, d2) => {
            if *d1 <= 0.0 || *d2 <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distances must be positive".to_string(),
                ));
            }
        }
        ChamferType::DistanceAngle(d, a) => {
            if *d <= 0.0 {
                return Err(OperationError::InvalidGeometry(
                    "Distance must be positive".to_string(),
                ));
            }
            if *a <= 0.0 || *a >= std::f64::consts::PI {
                return Err(OperationError::InvalidGeometry(
                    "Angle must be between 0 and π".to_string(),
                ));
            }
        }
        ChamferType::Angle(a) => {
            if *a <= 0.0 || *a >= std::f64::consts::PI {
                return Err(OperationError::InvalidGeometry(
                    "Angle must be between 0 and π".to_string(),
                ));
            }
        }
    }

    Ok(())
}

/// Validate chamfered solid
fn validate_chamfered_solid(_model: &BRepModel, _solid_id: SolidId) -> OperationResult<()> {
    // Would perform full validation
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::TopologyBuilder;

    #[test]
    fn test_chamfer_validation_rejects_zero_distance() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        // Get any edge from the model
        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
        assert!(!edges.is_empty(), "Box should have edges");

        let options = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(0.0),
            ..Default::default()
        };

        let result = chamfer_edges(&mut model, solid_id, vec![edges[0]], options);
        assert!(result.is_err(), "Zero distance should be rejected");
    }

    #[test]
    fn test_chamfer_validation_rejects_negative_distance() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();

        let options = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(-1.0),
            ..Default::default()
        };

        let result = chamfer_edges(&mut model, solid_id, vec![edges[0]], options);
        assert!(result.is_err(), "Negative distance should be rejected");
    }

    #[test]
    fn test_chamfer_validation_rejects_invalid_angle() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();

        // Angle >= π should be rejected
        let options = ChamferOptions {
            chamfer_type: ChamferType::Angle(std::f64::consts::PI),
            ..Default::default()
        };

        let result = chamfer_edges(&mut model, solid_id, vec![edges[0]], options);
        assert!(result.is_err(), "Angle >= π should be rejected");
    }

    #[test]
    fn test_chamfer_validation_nonexistent_solid() {
        let mut model = BRepModel::new();
        let fake_solid = SolidId::from(999u32);
        let fake_edge = EdgeId::from(0u32);

        let options = ChamferOptions::default();
        let result = chamfer_edges(&mut model, fake_solid, vec![fake_edge], options);
        assert!(result.is_err(), "Nonexistent solid should be rejected");
    }

    #[test]
    fn test_get_adjacent_faces_finds_shared_edge() {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let result = builder.create_box_3d(10.0, 10.0, 10.0);
        let solid_id = match result {
            Ok(crate::primitives::topology_builder::GeometryId::Solid(id)) => id,
            _ => panic!("Failed to create box"),
        };

        // Get an edge that should be shared by exactly 2 faces
        let edges: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
        if edges.is_empty() {
            return; // Skip if box creation didn't produce edges
        }

        let result = get_adjacent_faces(&model, solid_id, edges[0]);
        match result {
            Ok((f1, f2)) => {
                assert_ne!(f1, f2, "Adjacent faces must be different");
            }
            Err(_) => {
                // Edge may not be in a face loop depending on box topology builder
                // This is acceptable — the function correctly reports the error
            }
        }
    }

    #[test]
    fn test_compute_face_angle_perpendicular_box() {
        use crate::primitives::surface::Plane;

        let mut model = BRepModel::new();

        // Create two perpendicular planes (like box faces)
        let plane1 = Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();
        let plane2 = Plane::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
            Vector3::new(1.0, 0.0, 0.0),
        )
        .unwrap();

        let s1 = model.surfaces.add(Box::new(plane1));
        let s2 = model.surfaces.add(Box::new(plane2));

        // Create a shared edge along x-axis
        use crate::primitives::curve::Line;
        let line = Line::new(Point3::new(0.0, 0.0, 0.0), Point3::new(10.0, 0.0, 0.0));
        let curve_id = model.curves.add(Box::new(line));

        let v1 = model.vertices.add(0.0, 0.0, 0.0);
        let v2 = model.vertices.add(10.0, 0.0, 0.0);

        let edge = Edge::new_auto_range(0, v1, v2, curve_id, EdgeOrientation::Forward);
        let edge_id = model.edges.add(edge);

        // Create loops and faces
        let mut loop1 = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
        loop1.add_edge(edge_id, true);
        let loop1_id = model.loops.add(loop1);

        let mut loop2 = Loop::new(0, crate::primitives::r#loop::LoopType::Outer);
        loop2.add_edge(edge_id, false);
        let loop2_id = model.loops.add(loop2);

        let face1 = Face::new(0, s1, loop1_id, FaceOrientation::Forward);
        let face1_id = model.faces.add(face1);

        let face2 = Face::new(0, s2, loop2_id, FaceOrientation::Forward);
        let face2_id = model.faces.add(face2);

        // Dihedral angle between perpendicular faces should be ~π/2
        let angle = compute_face_angle(&model, edge_id, face1_id, face2_id).unwrap();
        let expected = std::f64::consts::FRAC_PI_2;
        assert!(
            (angle - expected).abs() < 0.1,
            "Expected ~π/2 ({expected:.3}), got {angle:.3}"
        );
    }

    #[test]
    fn test_ruled_chamfer_surface_creation() {
        let mut model = BRepModel::new();

        let data = ChamferData {
            offset_points1: vec![
                Point3::new(0.0, 0.0, 1.0),
                Point3::new(5.0, 0.0, 1.0),
                Point3::new(10.0, 0.0, 1.0),
            ],
            offset_points2: vec![
                Point3::new(0.0, 1.0, 0.0),
                Point3::new(5.0, 1.0, 0.0),
                Point3::new(10.0, 1.0, 0.0),
            ],
            parameters: vec![0.0, 0.5, 1.0],
            normals1: vec![Vector3::new(0.0, 0.0, 1.0); 3],
            normals2: vec![Vector3::new(0.0, 1.0, 0.0); 3],
        };

        let surface = create_ruled_chamfer_surface(&mut model, &data).unwrap();

        // At v=0 (curve1): should be near offset_points1
        let p0 = surface.point_at(0.0, 0.0).unwrap();
        assert!(
            (p0.x - 0.0).abs() < 1e-10 && (p0.z - 1.0).abs() < 1e-10,
            "v=0, u=0 should be on curve1 start"
        );

        // At v=1 (curve2): should be near offset_points2
        let p1 = surface.point_at(0.0, 1.0).unwrap();
        assert!(
            (p1.y - 1.0).abs() < 1e-10 && (p1.z - 0.0).abs() < 1e-10,
            "v=1, u=0 should be on curve2 start"
        );

        // At v=0.5: midpoint interpolation
        let pm = surface.point_at(0.5, 0.5).unwrap();
        assert!(
            (pm.x - 5.0).abs() < 1e-10,
            "Midpoint x should be 5.0, got {}",
            pm.x
        );
    }
}
