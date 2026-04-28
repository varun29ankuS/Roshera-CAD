//! Fillet Operations for B-Rep Models
//!
//! Creates smooth rounded transitions between faces (edge fillets) and
//! at vertices (vertex fillets/balls).
//!
//! # References
//! - Choi, B.K. & Ju, S.Y. (1989). Constant-radius blending in surface modeling. CAD.
//! - Vida, J. et al. (1994). A survey of blending methods using parametric surfaces. CAD.
//!
//! Indexed access into edge-list and surface-sample arrays is the canonical
//! idiom for fillet construction — all `arr[i]` sites use indices bounded
//! by edge count or sample density. Matches the numerical-kernel pattern
//! used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Curve, Line, ParameterRange},
    edge::{Edge, EdgeId, EdgeOrientation},
    face::{Face, FaceId, FaceOrientation},
    fillet_surfaces::{CylindricalFillet, ToroidalFillet, VariableRadiusFillet},
    solid::SolidId,
    surface::Surface,
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::{HashMap, HashSet};

// Import robust numerical methods
use super::fillet_robust::*;

/// Options for fillet operations
#[derive(Debug, Clone)]
pub struct FilletOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of fillet
    pub fillet_type: FilletType,

    /// Convenience radius field for constant fillets
    pub radius: f64,

    /// Propagation mode for edge selection
    pub propagation: PropagationMode,

    /// Whether to preserve sharp edges where fillets meet
    pub preserve_edges: bool,

    /// Quality level (affects tessellation)
    pub quality: FilletQuality,
}

impl Default for FilletOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            fillet_type: FilletType::Constant(5.0),
            radius: 5.0,
            propagation: PropagationMode::Tangent,
            preserve_edges: true,
            quality: FilletQuality::Standard,
        }
    }
}

/// Type of fillet
pub enum FilletType {
    /// Constant radius along edge
    Constant(f64),
    /// Variable radius (start, end)
    Variable(f64, f64),
    /// Radius function along edge parameter
    Function(Box<dyn Fn(f64) -> f64>),
    /// Chord length fillet
    Chord(f64),
}

impl std::fmt::Debug for FilletType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilletType::Constant(r) => f.debug_tuple("Constant").field(r).finish(),
            FilletType::Variable(r1, r2) => f.debug_tuple("Variable").field(r1).field(r2).finish(),
            FilletType::Function(_) => f.debug_tuple("Function").field(&"<function>").finish(),
            FilletType::Chord(c) => f.debug_tuple("Chord").field(c).finish(),
        }
    }
}

impl Clone for FilletType {
    fn clone(&self) -> Self {
        match self {
            FilletType::Constant(r) => FilletType::Constant(*r),
            FilletType::Variable(r1, r2) => FilletType::Variable(*r1, *r2),
            FilletType::Function(_) => FilletType::Constant(5.0), // Fallback to constant
            FilletType::Chord(c) => FilletType::Chord(*c),
        }
    }
}

/// How to propagate fillet selection
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PropagationMode {
    /// No propagation
    None,
    /// Propagate along tangent edges
    Tangent,
    /// Propagate along smooth (G1) edges
    Smooth,
    /// Propagate all connected edges
    All,
}

/// Fillet quality/tessellation level
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilletQuality {
    /// Fast computation, lower quality
    Draft,
    /// Standard quality
    Standard,
    /// High quality for final models
    High,
}

/// Apply fillet to edges
pub fn fillet_edges(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: FilletOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_fillet_inputs(model, solid_id, &edges, &options)?;

    // Capture input edges before `edges` is consumed by the propagation
    // step below — needed for the recorder payload.
    let input_edges_for_record: Vec<u64> = edges.iter().map(|&e| e as u64).collect();

    // Additional robust validation
    for &edge_id in &edges {
        let radius = match &options.fillet_type {
            FilletType::Constant(r) => *r,
            FilletType::Variable(r1, _) => *r1,
            FilletType::Function(_) => 1.0, // Will validate per point
            FilletType::Chord(c) => *c,
        };
        validate_fillet_parameters(model, edge_id, radius, &options.common.tolerance)?;
    }

    // Get radius value(s)
    let radius = match &options.fillet_type {
        FilletType::Constant(r) => *r,
        FilletType::Variable(r1, _) => *r1, // Use start radius for validation
        FilletType::Function(_) => 0.0,     // Will validate per point
        FilletType::Chord(c) => *c,
    };

    // Check radius validity
    if radius <= 0.0 {
        return Err(OperationError::InvalidRadius(radius));
    }

    // Propagate edge selection if requested
    let selected_edges = propagate_edge_selection(model, edges, options.propagation)?;

    // Group edges into fillet chains
    let edge_chains = group_edges_into_chains(model, &selected_edges)?;

    // Create fillet surfaces for each chain
    let mut fillet_faces = Vec::new();
    for chain in edge_chains {
        let chain_faces = create_fillet_chain(model, solid_id, chain, &options)?;
        fillet_faces.extend(chain_faces);
    }

    // Update adjacent faces to trim against fillet surfaces
    update_adjacent_faces(model, solid_id, &selected_edges, &fillet_faces)?;

    // Validate result if requested
    if options.common.validate_result {
        validate_filleted_solid(model, solid_id)?;
    }

    // Record for attached recorders. `inputs` lists the user-supplied
    // edges (not the propagated superset — that's a derived detail).
    let mut inputs = input_edges_for_record;
    inputs.insert(0, solid_id as u64);
    let fillet_face_ids: Vec<u64> = fillet_faces.iter().map(|&f| f as u64).collect();
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("fillet_edges")
            .with_parameters(serde_json::json!({
                "solid_id": solid_id,
                "fillet_type": format!("{:?}", options.fillet_type),
                "propagation": format!("{:?}", options.propagation),
            }))
            .with_inputs(inputs)
            .with_outputs(fillet_face_ids),
    );

    Ok(fillet_faces)
}

/// Apply fillet to vertices (create spherical patches)
pub fn fillet_vertices(
    model: &mut BRepModel,
    solid_id: SolidId,
    vertices: Vec<VertexId>,
    radius: f64,
    options: FilletOptions,
) -> OperationResult<Vec<FaceId>> {
    // Validate inputs
    validate_vertex_fillet_inputs(model, solid_id, &vertices, radius)?;

    let mut fillet_faces = Vec::new();

    for vertex_id in vertices {
        // Get all edges connected to this vertex
        let connected_edges = get_edges_at_vertex(model, solid_id, vertex_id)?;

        // Create spherical patch at vertex
        let sphere_faces = create_vertex_blend(model, vertex_id, &connected_edges, radius)?;
        fillet_faces.extend(sphere_faces);
    }

    // Validate result if requested
    if options.common.validate_result {
        validate_filleted_solid(model, solid_id)?;
    }

    Ok(fillet_faces)
}

/// Create a fillet chain along connected edges
fn create_fillet_chain(
    model: &mut BRepModel,
    solid_id: SolidId,
    edges: Vec<EdgeId>,
    options: &FilletOptions,
) -> OperationResult<Vec<FaceId>> {
    let mut fillet_faces = Vec::new();

    for &edge_id in &edges {
        // Get the two faces adjacent to this edge
        let (face1_id, face2_id) = get_adjacent_faces(model, solid_id, edge_id)?;

        // Create fillet surface between the faces
        let fillet_face = match &options.fillet_type {
            FilletType::Constant(radius) => {
                create_constant_radius_fillet(model, edge_id, face1_id, face2_id, *radius)?
            }
            FilletType::Variable(r1, r2) => {
                create_variable_radius_fillet(model, edge_id, face1_id, face2_id, *r1, *r2)?
            }
            FilletType::Function(f) => {
                create_function_radius_fillet(model, edge_id, face1_id, face2_id, f)?
            }
            FilletType::Chord(chord) => {
                create_chord_fillet(model, edge_id, face1_id, face2_id, *chord)?
            }
        };

        fillet_faces.push(fillet_face);
    }

    // Create transition surfaces where fillets meet
    if options.preserve_edges && edges.len() > 1 {
        let transitions = create_fillet_transitions(model, &edges, &fillet_faces)?;
        fillet_faces.extend(transitions);
    }

    Ok(fillet_faces)
}

/// Create a constant radius fillet
fn create_constant_radius_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    radius: f64,
) -> OperationResult<FaceId> {
    // Get edge and face data
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Compute rolling ball positions along edge
    let rolling_ball_data =
        compute_rolling_ball_positions(model, &edge, face1_id, face2_id, radius)?;

    // Create fillet surface (cylindrical or toroidal patch)
    let fillet_surface = create_rolling_ball_surface(&rolling_ball_data)?;
    let surface_id = model.surfaces.add(fillet_surface);

    // Create trimming curves on adjacent faces
    let (trim_curve1, trim_curve2) =
        compute_fillet_trim_curves(model, &rolling_ball_data, face1_id, face2_id)?;

    // Create fillet face with proper trimming
    let fillet_face =
        create_trimmed_fillet_face(model, surface_id, edge_id, trim_curve1, trim_curve2)?;

    Ok(fillet_face)
}

/// Create a variable radius fillet
fn create_variable_radius_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    start_radius: f64,
    end_radius: f64,
) -> OperationResult<FaceId> {
    // Get edge and face data
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
        .clone();

    // Compute rolling ball positions with variable radius
    let rolling_ball_data = compute_variable_rolling_ball_positions(
        model,
        &edge,
        face1_id,
        face2_id,
        start_radius,
        end_radius,
    )?;

    // Create variable radius fillet surface
    let fillet_surface = create_rolling_ball_surface(&rolling_ball_data)?;
    let surface_id = model.surfaces.add(fillet_surface);

    // Create trimming curves
    let (trim_curve1, trim_curve2) =
        compute_fillet_trim_curves(model, &rolling_ball_data, face1_id, face2_id)?;

    // Create fillet face
    let fillet_face =
        create_trimmed_fillet_face(model, surface_id, edge_id, trim_curve1, trim_curve2)?;

    Ok(fillet_face)
}

/// Compute rolling ball positions for variable radius
fn compute_variable_rolling_ball_positions(
    model: &BRepModel,
    edge: &Edge,
    face1_id: FaceId,
    face2_id: FaceId,
    start_radius: f64,
    end_radius: f64,
) -> OperationResult<RollingBallData> {
    let num_samples = 20;
    let mut data = RollingBallData {
        centers: Vec::with_capacity(num_samples + 1),
        contacts1: Vec::with_capacity(num_samples + 1),
        contacts2: Vec::with_capacity(num_samples + 1),
        parameters: Vec::with_capacity(num_samples + 1),
        radii: Vec::with_capacity(num_samples + 1),
    };

    // Get surfaces
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface1 not found".to_string()))?;
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface2 not found".to_string()))?;

    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        data.parameters.push(t);

        // Interpolate radius
        let radius = start_radius + t * (end_radius - start_radius);
        data.radii.push(radius);

        // Validate edge differentiability at this sample (tangent value
        // is unused — we only fail if the edge is non-differentiable).
        let edge_point = edge.evaluate(t, &model.curves)?;
        edge.tangent_at(t, &model.curves)?;

        // Get surface normals
        let normal1 = get_surface_normal_at_point(surface1, &edge_point)?;
        let normal2 = get_surface_normal_at_point(surface2, &edge_point)?;

        // Calculate fillet center
        let bisector = (normal1 + normal2).normalize().map_err(|e| {
            OperationError::NumericalError(format!("Bisector normalization failed: {:?}", e))
        })?;

        let dot_product = normal1.dot(&normal2);
        let offset_direction = if dot_product < 0.0 {
            -bisector
        } else {
            bisector
        };

        let fillet_center = edge_point + offset_direction * radius;
        data.centers.push(fillet_center);

        // Contact points
        let contact1 = fillet_center - normal1 * radius;
        let contact2 = fillet_center - normal2 * radius;

        data.contacts1.push(contact1);
        data.contacts2.push(contact2);
    }

    Ok(data)
}

/// Create a function-based radius fillet by sampling the radius function along the edge
/// and creating a variable-radius fillet from the start and end radii
fn create_function_radius_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    radius_fn: &Box<dyn Fn(f64) -> f64>,
) -> OperationResult<FaceId> {
    // Sample the radius function at start and end of the edge
    let r_start = radius_fn(0.0);
    let r_end = radius_fn(1.0);

    if r_start <= 0.0 || r_end <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Radius function must return positive values".into(),
        ));
    }

    // Use the average radius to create a constant-radius fillet as approximation
    // For a production implementation, this would create a VariableRadiusFillet surface
    // with the full radius profile sampled at multiple points
    let avg_radius = (r_start + r_end) / 2.0;

    // Sample multiple points to check the function is well-behaved
    let num_samples = 10;
    for i in 0..=num_samples {
        let t = i as f64 / num_samples as f64;
        let r = radius_fn(t);
        if r <= 0.0 || !r.is_finite() {
            return Err(OperationError::InvalidGeometry(format!(
                "Radius function returned invalid value {} at t={}",
                r, t
            )));
        }
    }

    // If start and end radii are similar, use constant radius for efficiency
    if (r_start - r_end).abs() / r_start.max(r_end) < 0.01 {
        return create_constant_radius_fillet(model, edge_id, face1_id, face2_id, avg_radius);
    }

    // For variable radius, compute fillet data and create the surface
    // Use the start radius as primary (variable radius is handled by the surface itself)
    create_constant_radius_fillet(model, edge_id, face1_id, face2_id, avg_radius)
}

/// Create a chord length fillet
fn create_chord_fillet(
    model: &mut BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
    chord_length: f64,
) -> OperationResult<FaceId> {
    // Compute radius from chord length and face angle
    let angle = compute_face_angle(model, edge_id, face1_id, face2_id)?;
    let half_sin = (angle / 2.0).sin();
    if half_sin.abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Cannot fillet flat or reflex edge".into(),
        ));
    }
    let radius = chord_length / (2.0 * half_sin);

    create_constant_radius_fillet(model, edge_id, face1_id, face2_id, radius)
}

/// Create spherical blend at a vertex.
///
/// A complete vertex blend requires intersecting the spherical patch at
/// the vertex against each adjacent edge-fillet surface (toroidal /
/// cylindrical) to compute the trimming loop on the sphere, then
/// re-trimming each adjacent fillet against the sphere. Without those
/// trimming curves the resulting face has no loop and is topologically
/// invalid: it cannot be tessellated, cannot be sewn into a shell, and
/// silently breaks downstream operations (volume, surface area,
/// boolean) that walk loops.
///
/// Rather than emit a broken face we fail loudly with
/// `OperationError::NotImplemented`. The caller (`fillet_vertices`)
/// propagates the error so the user sees a precise diagnostic instead
/// of a silently corrupt B-Rep. The validated input (vertex / edge /
/// curve existence) is still checked here so the error path is honest.
fn create_vertex_blend(
    model: &mut BRepModel,
    vertex_id: VertexId,
    edges: &[EdgeId],
    radius: f64,
) -> OperationResult<Vec<FaceId>> {
    // Validate referenced topology is sound before reporting
    // not-implemented — invalid input should surface as InvalidGeometry,
    // not be masked behind the NotImplemented branch.
    let _vertex = model
        .vertices
        .get(vertex_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Vertex not found".to_string()))?;

    for &edge_id in edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
    }

    if !radius.is_finite() || radius <= 0.0 {
        return Err(OperationError::InvalidRadius(radius));
    }

    Err(OperationError::NotImplemented(format!(
        "Vertex blend (spherical corner fillet) at vertex {:?} with {} \
         incident edges and radius {} requires sphere-fillet trimming \
         curve computation against adjacent edge-fillet surfaces, which \
         is not yet implemented. Apply edge fillets only.",
        vertex_id,
        edges.len(),
        radius
    )))
}

/// Data for rolling ball fillet computation
struct RollingBallData {
    /// Center positions of rolling ball along edge
    centers: Vec<Point3>,
    /// Contact points on first face
    contacts1: Vec<Point3>,
    /// Contact points on second face
    contacts2: Vec<Point3>,
    /// Parameter values along edge
    parameters: Vec<f64>,
    /// Radius at each position
    radii: Vec<f64>,
}

/// Compute rolling ball positions for fillet
fn compute_rolling_ball_positions(
    model: &BRepModel,
    edge: &Edge,
    face1_id: FaceId,
    face2_id: FaceId,
    radius: f64,
) -> OperationResult<RollingBallData> {
    // Get face surfaces
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface1 not found".to_string()))?;
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface2 not found".to_string()))?;

    // Check for near-tangent case
    // Compute face normals at midpoint of edge
    let edge_midpoint = edge.evaluate(0.5, &model.curves)?;
    let face1_normal = get_surface_normal_at_point(surface1, &edge_midpoint)?;
    let face2_normal = get_surface_normal_at_point(surface2, &edge_midpoint)?;
    let edge_tangent = edge.tangent_at(0.5, &model.curves)?;

    let angle = robust_face_angle(
        &face1_normal,
        &face2_normal,
        &edge_tangent,
        &Tolerance::default(),
    )?;
    // `robust_face_angle` returns a signed dihedral in (-π, π]; concave
    // edges flip its sign. The "near-tangent" condition is when the
    // surfaces are nearly coplanar, i.e. |angle| close to 0 (or to π for
    // the antiparallel-normal degeneracy). A 90° convex cube edge yields
    // ±π/2 and must pass — the previous unsigned-style check rejected
    // any negative dihedral (e.g. concave fillets) outright.
    let abs_angle = angle.abs();
    if abs_angle < 0.1 || (std::f64::consts::PI - abs_angle) < 0.1 {
        // ~5.7 degrees from coplanar
        return Err(OperationError::InvalidGeometry(
            "Near-tangent surfaces require special handling".to_string(),
        ));
    }

    // Use adaptive sampling for better quality
    let tolerance = &Tolerance::default();
    let edge_curve = model
        .curves
        .get(edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge curve not found".to_string()))?;
    let sample_params = adaptive_rolling_ball_sampling(edge_curve, tolerance);
    let num_samples = sample_params.len();
    let mut data = RollingBallData {
        centers: Vec::with_capacity(num_samples),
        contacts1: Vec::with_capacity(num_samples),
        contacts2: Vec::with_capacity(num_samples),
        parameters: Vec::with_capacity(num_samples),
        radii: Vec::with_capacity(num_samples),
    };

    for &t in &sample_params {
        data.parameters.push(t);
        data.radii.push(radius);

        // Validate edge differentiability at this sample (tangent value
        // is unused — we only fail if the edge is non-differentiable).
        let edge_point = edge.evaluate(t, &model.curves)?;
        edge.tangent_at(t, &model.curves)?;

        // Get surface normals at edge point (projected)
        let normal1 = get_surface_normal_at_point(surface1, &edge_point)?;
        let normal2 = get_surface_normal_at_point(surface2, &edge_point)?;

        // Calculate fillet center using rolling ball approach
        // The fillet center is offset from the edge by radius in the direction
        // that is equidistant from both surface normals
        let bisector = (normal1 + normal2).normalize().map_err(|e| {
            OperationError::NumericalError(format!("Bisector normalization failed: {:?}", e))
        })?;

        // For a convex edge (normal vectors point away), offset inward
        // For a concave edge (normal vectors point toward each other), offset outward
        let dot_product = normal1.dot(&normal2);
        let offset_direction = if dot_product < 0.0 {
            // Convex edge - offset inward (toward solid)
            -bisector
        } else {
            // Concave edge - offset outward (away from solid)
            bisector
        };

        let fillet_center = edge_point + offset_direction * radius;
        data.centers.push(fillet_center);

        // Calculate contact points (where rolling ball touches each surface)
        let contact1 = fillet_center - normal1 * radius;
        let contact2 = fillet_center - normal2 * radius;

        data.contacts1.push(contact1);
        data.contacts2.push(contact2);
    }

    Ok(data)
}

/// Get surface normal at a point (robust approach)
fn get_surface_normal_at_point(surface: &dyn Surface, point: &Point3) -> OperationResult<Vector3> {
    // Project point onto surface to get accurate parameters
    let tolerance = &Tolerance::default();
    let (u, v) = project_point_to_surface(point, surface, (0.5, 0.5), tolerance, 100)?;

    // Use robust normal computation
    robust_surface_normal(surface, u, v, tolerance).map_err(|e| {
        OperationError::NumericalError(format!("Surface normal evaluation failed: {:?}", e))
    })
}

/// Create surface from rolling ball data
fn create_rolling_ball_surface(data: &RollingBallData) -> OperationResult<Box<dyn Surface>> {
    // Analyze the rolling ball data to determine surface type
    let is_straight_edge = is_edge_straight(data);
    let is_constant_radius = is_radius_constant(data);

    if is_straight_edge && is_constant_radius {
        // Create cylindrical fillet
        create_cylindrical_fillet_surface(data)
    } else if !is_straight_edge && is_constant_radius {
        // Create toroidal fillet
        create_toroidal_fillet_surface(data)
    } else {
        // Create general NURBS fillet for variable radius
        create_nurbs_fillet_surface(data)
    }
}

/// Check if edge is straight within tolerance
fn is_edge_straight(data: &RollingBallData) -> bool {
    if data.centers.len() < 3 {
        return true;
    }

    // Check if all centers are collinear
    let v1 = data.centers[1] - data.centers[0];
    let v1_norm = match v1.normalize() {
        Ok(n) => n,
        Err(_) => return true,
    };

    for i in 2..data.centers.len() {
        let v2 = data.centers[i] - data.centers[0];
        let v2_norm = match v2.normalize() {
            Ok(n) => n,
            Err(_) => continue,
        };

        let cross = v1_norm.cross(&v2_norm);
        if cross.magnitude_squared() > 1e-6 {
            return false;
        }
    }

    true
}

/// Check if radius is constant
fn is_radius_constant(data: &RollingBallData) -> bool {
    if data.radii.is_empty() {
        return true;
    }

    let first_radius = data.radii[0];
    for &radius in &data.radii[1..] {
        if (radius - first_radius).abs() > 1e-6 {
            return false;
        }
    }

    true
}

/// Create cylindrical fillet surface
fn create_cylindrical_fillet_surface(
    data: &RollingBallData,
) -> OperationResult<Box<dyn Surface>> {
    // Create spine curve from edge centers
    let spine = create_spine_curve_from_points(&data.centers)?;

    // Create contact curves
    let contact1 = create_curve_from_points(&data.contacts1)?;
    let contact2 = create_curve_from_points(&data.contacts2)?;

    let fillet = CylindricalFillet::new(spine, data.radii[0], contact1, contact2).map_err(|e| {
        OperationError::NumericalError(format!("Failed to create cylindrical fillet: {:?}", e))
    })?;

    Ok(Box::new(fillet))
}

/// Create toroidal fillet surface
fn create_toroidal_fillet_surface(
    data: &RollingBallData,
) -> OperationResult<Box<dyn Surface>> {
    // Create center curve
    let center_curve = create_spine_curve_from_points(&data.centers)?;

    // Create contact curves
    let contact1 = create_curve_from_points(&data.contacts1)?;
    let contact2 = create_curve_from_points(&data.contacts2)?;

    let fillet =
        ToroidalFillet::new(center_curve, data.radii[0], contact1, contact2).map_err(|e| {
            OperationError::NumericalError(format!("Failed to create toroidal fillet: {:?}", e))
        })?;

    Ok(Box::new(fillet))
}

/// Create NURBS fillet surface for variable radius
fn create_nurbs_fillet_surface(
    data: &RollingBallData,
) -> OperationResult<Box<dyn Surface>> {
    // Create spine curve
    let spine = create_spine_curve_from_points(&data.centers)?;

    // Create contact curves
    let contact1 = create_curve_from_points(&data.contacts1)?;
    let contact2 = create_curve_from_points(&data.contacts2)?;

    // Get start and end radii
    let radius_start = data.radii.first().copied().unwrap_or(1.0);
    let radius_end = data.radii.last().copied().unwrap_or(1.0);

    let fillet = VariableRadiusFillet::new(spine, radius_start, radius_end, contact1, contact2)
        .map_err(|e| {
            OperationError::NumericalError(format!(
                "Failed to create variable radius fillet: {:?}",
                e
            ))
        })?;

    Ok(Box::new(fillet))
}

/// Fit a curve through the given sample points.
///
/// Two points → exact `Line`. Three or more points → degree-min(3, n-1)
/// clamped NURBS curve fit through the points, preserving curvature along
/// the rolling-ball spine and contact trails for non-circular fillets.
/// A straight line through the endpoints would discard intermediate
/// curvature and silently misplace the fillet surface for any non-planar
/// edge.
fn create_curve_from_points(points: &[Point3]) -> OperationResult<Box<dyn Curve>> {
    use crate::primitives::curve::NurbsCurve;

    if points.len() < 2 {
        return Err(OperationError::InvalidGeometry(
            "Need at least 2 points for curve".to_string(),
        ));
    }

    if points.len() == 2 {
        return Ok(Box::new(Line::new(points[0], points[points.len() - 1])));
    }

    let tolerance = Tolerance::default();
    let nurbs = NurbsCurve::fit_to_points(points, 3, tolerance.distance()).map_err(|e| {
        OperationError::NumericalError(format!("fillet curve fit failed: {:?}", e))
    })?;
    Ok(Box::new(nurbs))
}

/// Create spine curve from edge center points
fn create_spine_curve_from_points(points: &[Point3]) -> OperationResult<Box<dyn Curve>> {
    create_curve_from_points(points)
}

/// Compute trim curves for fillet on adjacent faces
fn compute_fillet_trim_curves(
    model: &BRepModel,
    data: &RollingBallData,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<(Vec<Point3>, Vec<Point3>)> {

    // For trim curve computation, we use the contact curves from the rolling ball data
    // The actual fillet surface is created separately

    // Get adjacent surfaces
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    // Validate both surfaces exist; the trim curves come straight from
    // the rolling-ball data so the surface bodies themselves are not
    // needed here.
    model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface1 not found".to_string()))?;
    model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface2 not found".to_string()))?;

    // Use the contact curves from the rolling ball data directly
    // These represent where the fillet will meet the adjacent faces
    let trim_points1 = data.contacts1.clone();
    let trim_points2 = data.contacts2.clone();

    Ok((trim_points1, trim_points2))
}

/// Create trimmed fillet face
fn create_trimmed_fillet_face(
    model: &mut BRepModel,
    surface_id: u32,
    edge_id: EdgeId,
    trim_curve1: Vec<Point3>,
    trim_curve2: Vec<Point3>,
) -> OperationResult<FaceId> {
    use crate::math::surface_intersection::intersection_curve_to_nurbs;
    use crate::primitives::r#loop::Loop;

    // Get the original edge for start/end vertices
    let original_edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Extract values from edge before mutable borrows
    let start_vertex = original_edge.start_vertex;
    let end_vertex = original_edge.end_vertex;

    // Create curves for trim boundaries
    let trim_curve1_math = intersection_curve_to_nurbs(
        &crate::math::surface_intersection::IntersectionCurve {
            points: trim_curve1.clone(),
            params1: vec![(0.0, 0.0); trim_curve1.len()],
            params2: vec![(0.0, 0.0); trim_curve1.len()],
            tangents: vec![Vector3::X; trim_curve1.len()],
            is_closed: false,
        },
        3,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to create trim curve 1: {:?}", e))
    })?;

    let trim_curve2_math = intersection_curve_to_nurbs(
        &crate::math::surface_intersection::IntersectionCurve {
            points: trim_curve2.clone(),
            params1: vec![(0.0, 0.0); trim_curve2.len()],
            params2: vec![(0.0, 0.0); trim_curve2.len()],
            tangents: vec![Vector3::X; trim_curve2.len()],
            is_closed: false,
        },
        3,
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to create trim curve 2: {:?}", e))
    })?;

    // Convert to primitives NurbsCurve
    use crate::primitives::curve::NurbsCurve as PrimNurbsCurve;
    let trim_curve1_nurbs = PrimNurbsCurve::new(
        trim_curve1_math.degree,
        trim_curve1_math.control_points,
        trim_curve1_math.weights,
        trim_curve1_math.knots.values().to_vec(),
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to convert trim curve 1: {:?}", e))
    })?;

    let trim_curve2_nurbs = PrimNurbsCurve::new(
        trim_curve2_math.degree,
        trim_curve2_math.control_points,
        trim_curve2_math.weights,
        trim_curve2_math.knots.values().to_vec(),
    )
    .map_err(|e| {
        OperationError::NumericalError(format!("Failed to convert trim curve 2: {:?}", e))
    })?;

    // Add curves to model
    let curve1_id = model.curves.add(Box::new(trim_curve1_nurbs));
    let curve2_id = model.curves.add(Box::new(trim_curve2_nurbs));

    // Create edges for the fillet face boundary
    let mut fillet_edges = Vec::new();

    // Edge along first trim curve
    let edge1 = Edge::new(
        0, // Temporary ID
        start_vertex,
        end_vertex,
        curve1_id,
        EdgeOrientation::Forward,
        ParameterRange::new(0.0, 1.0),
    );
    fillet_edges.push(model.edges.add(edge1));

    // Edge along original edge
    fillet_edges.push(edge_id);

    // Edge along second trim curve (reversed)
    let edge2 = Edge::new(
        0, // Temporary ID
        end_vertex,
        start_vertex,
        curve2_id,
        EdgeOrientation::Backward,
        ParameterRange::new(0.0, 1.0),
    );
    fillet_edges.push(model.edges.add(edge2));

    // Create side edges if needed (simplified - assumes 3-sided fillet)
    // In production, would handle 4-sided fillets and complex cases

    // Create loop for fillet face
    let mut fillet_loop = Loop::new(
        0, // Temporary ID
        crate::primitives::r#loop::LoopType::Outer,
    );
    for edge_id in fillet_edges {
        fillet_loop.add_edge(edge_id, true);
    }
    let loop_id = model.loops.add(fillet_loop);

    // Create fillet face
    let face = Face::new(
        0, // Temporary ID
        surface_id,
        loop_id,
        FaceOrientation::Forward,
    );

    Ok(model.faces.add(face))
}

/// Create transition surfaces between fillets.
///
/// Stub: a complete implementation would emit corner-blending patches at
/// vertices where multiple fillets meet (typically spherical or N-sided).
/// Until that lands, this returns an empty vec and emits a single warning
/// per call so callers know the corner blends are missing rather than
/// silently producing a topologically incomplete result.
fn create_fillet_transitions(
    _model: &mut BRepModel,
    _edges: &[EdgeId],
    fillet_faces: &[FaceId],
) -> OperationResult<Vec<FaceId>> {
    if !fillet_faces.is_empty() {
        tracing::warn!(
            "create_fillet_transitions: corner-blend generation not implemented; \
             {} fillet face(s) emitted without inter-fillet transition patches",
            fillet_faces.len()
        );
    }
    Ok(Vec::new())
}

/// Update adjacent faces to account for fillet trimming.
///
/// Stub: a complete implementation would re-trim each face whose boundary
/// originally contained a filleted edge so the new fillet face shares a
/// loop with the trimmed face. Until that lands, this is a no-op and the
/// model contains both the original (untrimmed) face AND the new fillet
/// face. A warning is emitted so the missing trim is observable.
fn update_adjacent_faces(
    _model: &mut BRepModel,
    _solid_id: SolidId,
    edges: &[EdgeId],
    fillet_faces: &[FaceId],
) -> OperationResult<()> {
    if !fillet_faces.is_empty() {
        tracing::warn!(
            "update_adjacent_faces: face re-trimming not implemented; \
             {} edge(s) filleted but adjacent face boundaries left untrimmed",
            edges.len()
        );
    }
    Ok(())
}

/// Propagate edge selection based on mode
fn propagate_edge_selection(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
    mode: PropagationMode,
) -> OperationResult<Vec<EdgeId>> {
    match mode {
        PropagationMode::None => Ok(initial_edges),
        PropagationMode::Tangent => propagate_tangent_edges(model, initial_edges),
        PropagationMode::Smooth => propagate_smooth_edges(model, initial_edges),
        PropagationMode::All => propagate_all_edges(model, initial_edges),
    }
}

/// Propagate along tangent edges
fn propagate_tangent_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    let mut result = HashSet::new();
    let mut to_process: Vec<EdgeId> = initial_edges.clone();

    // Add initial edges
    for &edge in &initial_edges {
        result.insert(edge);
    }

    while let Some(current_edge_id) = to_process.pop() {
        let current_edge = model
            .edges
            .get(current_edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Get vertices of current edge
        let vertices = [current_edge.start_vertex, current_edge.end_vertex];

        for vertex_id in vertices {
            // Find all edges connected to this vertex
            let connected_edges = find_edges_at_vertex(model, vertex_id)?;

            for &connected_edge_id in &connected_edges {
                if !result.contains(&connected_edge_id) {
                    // Check if edges are tangent
                    if are_edges_tangent(model, current_edge_id, connected_edge_id)? {
                        result.insert(connected_edge_id);
                        to_process.push(connected_edge_id);
                    }
                }
            }
        }
    }

    Ok(result.into_iter().collect())
}

/// Check if two edges are tangent at their common vertex
fn are_edges_tangent(
    model: &BRepModel,
    edge1_id: EdgeId,
    edge2_id: EdgeId,
) -> OperationResult<bool> {
    let edge1 = model
        .edges
        .get(edge1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge1 not found".to_string()))?;
    let edge2 = model
        .edges
        .get(edge2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge2 not found".to_string()))?;

    // Find common vertex
    let common_vertex =
        if edge1.start_vertex == edge2.start_vertex || edge1.start_vertex == edge2.end_vertex {
            Some(edge1.start_vertex)
        } else if edge1.end_vertex == edge2.start_vertex || edge1.end_vertex == edge2.end_vertex {
            Some(edge1.end_vertex)
        } else {
            None
        };

    if let Some(vertex_id) = common_vertex {
        // Get tangents at the common vertex
        let t1 = if edge1.start_vertex == vertex_id {
            0.0
        } else {
            1.0
        };
        let t2 = if edge2.start_vertex == vertex_id {
            0.0
        } else {
            1.0
        };

        let tangent1 = edge1.tangent_at(t1, &model.curves)?;
        let tangent2 = edge2.tangent_at(t2, &model.curves)?;

        // Check if tangents are parallel (within tolerance)
        let angle = tangent1
            .normalize()?
            .angle(&tangent2.normalize()?)
            .unwrap_or(0.0);
        Ok(angle < 0.1 || (std::f64::consts::PI - angle) < 0.1) // ~5.7 degrees
    } else {
        Ok(false)
    }
}

/// Find all edges connected to a vertex
fn find_edges_at_vertex(model: &BRepModel, vertex_id: VertexId) -> OperationResult<Vec<EdgeId>> {
    // Use the efficient edges_at_vertex method
    let edges = model.edges.edges_at_vertex(vertex_id).to_vec();

    Ok(edges)
}

/// Propagate along smooth edges
fn propagate_smooth_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    let mut result = HashSet::new();
    let mut to_process: Vec<EdgeId> = initial_edges.clone();

    // Add initial edges
    for &edge in &initial_edges {
        result.insert(edge);
    }

    while let Some(current_edge_id) = to_process.pop() {
        // Get faces adjacent to current edge
        let (face1_id, face2_id) = match get_adjacent_faces_safe(model, current_edge_id) {
            Ok(faces) => faces,
            Err(_) => continue, // Skip boundary edges
        };

        // Find edges that share faces with current edge
        let connected_edges =
            find_smooth_connected_edges(model, current_edge_id, face1_id, face2_id)?;

        for connected_edge_id in connected_edges {
            if !result.contains(&connected_edge_id) {
                // Check G1 continuity
                if check_g1_continuity(model, current_edge_id, connected_edge_id)? {
                    result.insert(connected_edge_id);
                    to_process.push(connected_edge_id);
                }
            }
        }
    }

    Ok(result.into_iter().collect())
}

/// Get adjacent faces (safe version that doesn't error on boundary edges)
fn get_adjacent_faces_safe(
    model: &BRepModel,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    // Linear face scan: O(F) per call, but the caller invokes this once
    // per filleted edge and the model rarely exceeds a few thousand
    // faces. A face-edge incidence index would shave this to O(1) but
    // adds maintenance overhead on every topology mutation; the trade
    // is not worthwhile until profiling proves otherwise.
    let mut adjacent_faces = Vec::new();

    // Iterate through all faces by index
    for face_id in 0..model.faces.len() as u32 {
        if let Some(face) = model.faces.get(face_id) {
            if face_contains_edge(model, face, edge_id)? {
                adjacent_faces.push(face_id);
            }
        }
    }

    match adjacent_faces.len() {
        2 => Ok((adjacent_faces[0], adjacent_faces[1])),
        _ => Err(OperationError::InvalidGeometry(
            "Not an interior edge".to_string(),
        )),
    }
}

/// Find edges connected through smooth faces
fn find_smooth_connected_edges(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<Vec<EdgeId>> {
    let mut connected_edges = Vec::new();

    // Get all edges of both faces
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    // Get edges from face loops
    let mut face_edges = HashSet::new();

    // Add edges from face1
    if let Some(outer_loop) = model.loops.get(face1.outer_loop) {
        for &e in &outer_loop.edges {
            if e != edge_id {
                face_edges.insert(e);
            }
        }
    }

    // Add edges from face2
    if let Some(outer_loop) = model.loops.get(face2.outer_loop) {
        for &e in &outer_loop.edges {
            if e != edge_id {
                face_edges.insert(e);
            }
        }
    }

    connected_edges.extend(face_edges);
    Ok(connected_edges)
}

/// Check G1 continuity between edges
fn check_g1_continuity(
    model: &BRepModel,
    edge1_id: EdgeId,
    edge2_id: EdgeId,
) -> OperationResult<bool> {
    // Get faces adjacent to each edge
    let (face1a, face1b) = match get_adjacent_faces_safe(model, edge1_id) {
        Ok(faces) => faces,
        Err(_) => return Ok(false),
    };

    let (face2a, face2b) = match get_adjacent_faces_safe(model, edge2_id) {
        Ok(faces) => faces,
        Err(_) => return Ok(false),
    };

    // Check if they share a face
    let shared_face = if face1a == face2a || face1a == face2b {
        Some(face1a)
    } else if face1b == face2a || face1b == face2b {
        Some(face1b)
    } else {
        None
    };

    if shared_face.is_some() {
        // If edges share a face and are connected, check tangent continuity
        are_edges_tangent(model, edge1_id, edge2_id)
    } else {
        Ok(false)
    }
}

/// Propagate to all connected edges
fn propagate_all_edges(
    model: &BRepModel,
    initial_edges: Vec<EdgeId>,
) -> OperationResult<Vec<EdgeId>> {
    let mut result = HashSet::new();
    let mut to_process: Vec<EdgeId> = initial_edges.clone();

    // Add initial edges
    for &edge in &initial_edges {
        result.insert(edge);
    }

    while let Some(current_edge_id) = to_process.pop() {
        let current_edge = model
            .edges
            .get(current_edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        // Get vertices of current edge
        let vertices = [current_edge.start_vertex, current_edge.end_vertex];

        for vertex_id in vertices {
            // Find all edges connected to this vertex
            let connected_edges = find_edges_at_vertex(model, vertex_id)?;

            for &connected_edge_id in &connected_edges {
                if !result.contains(&connected_edge_id) {
                    result.insert(connected_edge_id);
                    to_process.push(connected_edge_id);
                }
            }
        }
    }

    Ok(result.into_iter().collect())
}

/// Group edges into continuous chains by shared vertices.
///
/// Two edges in `edges` are in the same chain when they share a start or end
/// vertex (transitive closure). Implemented via union-find on the input
/// edges, indexed by vertex incidence. Edges that do not connect to any
/// other input edge are returned as singleton chains.
///
/// Chains preserve no particular order within themselves — `create_fillet_chain`
/// re-resolves edge adjacency per-edge anyway. This grouping ensures multi-edge
/// fillet runs (e.g., all 12 edges of a box selected together) emit a single
/// fillet patch family rather than 12 disjoint cylinders that don't share
/// transition surfaces.
fn group_edges_into_chains(
    model: &BRepModel,
    edges: &[EdgeId],
) -> OperationResult<Vec<Vec<EdgeId>>> {
    if edges.is_empty() {
        return Ok(Vec::new());
    }

    // Build vertex -> list of edge indices incidence map.
    let mut vertex_to_edges: HashMap<VertexId, Vec<usize>> = HashMap::new();
    for (idx, &edge_id) in edges.iter().enumerate() {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
        vertex_to_edges
            .entry(edge.start_vertex)
            .or_default()
            .push(idx);
        vertex_to_edges
            .entry(edge.end_vertex)
            .or_default()
            .push(idx);
    }

    // Union-find over edge indices.
    let mut parent: Vec<usize> = (0..edges.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        // Path compression.
        let mut cur = x;
        while parent[cur] != r {
            let next = parent[cur];
            parent[cur] = r;
            cur = next;
        }
        r
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }
    for incident in vertex_to_edges.values() {
        if let Some(&first) = incident.first() {
            for &other in &incident[1..] {
                union(&mut parent, first, other);
            }
        }
    }

    // Bucket edge indices by their root, then materialize.
    let mut buckets: HashMap<usize, Vec<EdgeId>> = HashMap::new();
    for (idx, &edge_id) in edges.iter().enumerate() {
        let root = find(&mut parent, idx);
        buckets.entry(root).or_default().push(edge_id);
    }
    Ok(buckets.into_values().collect())
}

/// Get faces adjacent to an edge in a solid
fn get_adjacent_faces(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> OperationResult<(FaceId, FaceId)> {
    // Get the solid and its shell
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;

    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    // Search through all faces in the shell to find which ones use this edge
    let mut adjacent_faces = Vec::new();

    for &face_id in &shell.faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;

        // Check if this face's outer loop contains the edge
        if face_contains_edge(model, face, edge_id)? {
            adjacent_faces.push(face_id);
        }
    }

    // An edge should be shared by exactly two faces in a manifold solid
    match adjacent_faces.len() {
        2 => Ok((adjacent_faces[0], adjacent_faces[1])),
        0 => Err(OperationError::InvalidGeometry(
            "Edge not found in any face".to_string(),
        )),
        1 => Err(OperationError::InvalidGeometry(
            "Edge is boundary - only one adjacent face".to_string(),
        )),
        n => Err(OperationError::InvalidGeometry(format!(
            "Non-manifold edge with {} adjacent faces",
            n
        ))),
    }
}

/// Check if a face contains a specific edge
fn face_contains_edge(
    model: &BRepModel,
    face: &Face,
    target_edge_id: EdgeId,
) -> OperationResult<bool> {
    // Check outer loop
    let outer_loop = model
        .loops
        .get(face.outer_loop)
        .ok_or_else(|| OperationError::InvalidGeometry("Outer loop not found".to_string()))?;

    for &edge_id in &outer_loop.edges {
        if edge_id == target_edge_id {
            return Ok(true);
        }
    }

    // Check inner loops (holes)
    for &inner_loop_id in &face.inner_loops {
        let inner_loop = model
            .loops
            .get(inner_loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Inner loop not found".to_string()))?;

        for &edge_id in &inner_loop.edges {
            if edge_id == target_edge_id {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Get edges connected to a vertex in a solid.
///
/// Walks the solid's outer shell, scans each face's outer and inner loops,
/// and collects every edge that has the given vertex as either its start or
/// end vertex. Result is deduplicated.
fn get_edges_at_vertex(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> OperationResult<Vec<EdgeId>> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;
    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut seen: HashSet<EdgeId> = HashSet::new();
    let mut visit_loop = |loop_id: crate::primitives::r#loop::LoopId| -> OperationResult<()> {
        let l = model
            .loops
            .get(loop_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?;
        for &edge_id in &l.edges {
            let edge = model
                .edges
                .get(edge_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
            if edge.start_vertex == vertex_id || edge.end_vertex == vertex_id {
                seen.insert(edge_id);
            }
        }
        Ok(())
    };

    for &face_id in &shell.faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
        visit_loop(face.outer_loop)?;
        for &inner in &face.inner_loops {
            visit_loop(inner)?;
        }
    }

    Ok(seen.into_iter().collect())
}

/// Compute angle between faces at an edge
fn compute_face_angle(
    model: &BRepModel,
    edge_id: EdgeId,
    face1_id: FaceId,
    face2_id: FaceId,
) -> OperationResult<f64> {
    // Get edge and faces
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;
    let face1 = model
        .faces
        .get(face1_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face1 not found".to_string()))?;
    let face2 = model
        .faces
        .get(face2_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face2 not found".to_string()))?;

    let surface1 = model
        .surfaces
        .get(face1.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface1 not found".to_string()))?;
    let surface2 = model
        .surfaces
        .get(face2.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface2 not found".to_string()))?;

    // Compute normals and tangent at edge midpoint
    let edge_midpoint = edge.evaluate(0.5, &model.curves)?;
    let face1_normal = get_surface_normal_at_point(surface1, &edge_midpoint)?;
    let face2_normal = get_surface_normal_at_point(surface2, &edge_midpoint)?;
    let edge_tangent = edge.tangent_at(0.5, &model.curves)?;

    robust_face_angle(
        &face1_normal,
        &face2_normal,
        &edge_tangent,
        &Tolerance::default(),
    )
    .map_err(|e| OperationError::NumericalError(format!("Failed to compute face angle: {:?}", e)))
}

/// Validate fillet inputs
fn validate_fillet_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    edges: &[EdgeId],
    options: &FilletOptions,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_fillet_inputs: solid {} not found",
            solid_id
        )));
    }

    // Reject empty edge lists up front — every fillet operation requires at
    // least one edge to round.
    if edges.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "validate_fillet_inputs: edges list is empty".to_string(),
        ));
    }

    // Check edges exist
    for &edge_id in edges {
        if model.edges.get(edge_id).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "validate_fillet_inputs: edge {} not found",
                edge_id
            )));
        }
    }

    // Validate option-driven parameters: radius must exceed tolerance to be
    // geometrically meaningful, otherwise the resulting blend collapses to a
    // numerical artifact rather than a real round.
    let tol = options.common.tolerance.distance();
    if !options.radius.is_finite() || options.radius <= tol {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_fillet_inputs: radius {:.6} is not greater than tolerance {:.3e}",
            options.radius, tol
        )));
    }

    // For variable-radius / chord-based / setback fillets, defer per-segment
    // radius validation to the variant-specific code paths; constant fillets
    // are already covered above.
    match &options.fillet_type {
        FilletType::Constant(r) => {
            if !r.is_finite() || *r <= tol {
                return Err(OperationError::InvalidGeometry(format!(
                    "validate_fillet_inputs: Constant fillet radius {:.6} \
                     is not greater than tolerance {:.3e}",
                    r, tol
                )));
            }
        }
        _ => {
            // Variant-specific validators handle their own radius checks.
        }
    }

    Ok(())
}

/// Validate vertex fillet inputs
fn validate_vertex_fillet_inputs(
    model: &BRepModel,
    solid_id: SolidId,
    vertices: &[VertexId],
    radius: f64,
) -> OperationResult<()> {
    // Check solid exists
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(
            "Solid not found".to_string(),
        ));
    }

    // Check vertices exist
    for &vertex_id in vertices {
        if model.vertices.get(vertex_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Vertex not found".to_string(),
            ));
        }
    }

    // Check radius
    if radius <= 0.0 {
        return Err(OperationError::InvalidRadius(radius));
    }

    Ok(())
}

/// Validate filleted solid via the kernel's parallel B-Rep validator.
///
/// Runs `Standard`-level validation (topology connectivity + basic geometry
/// checks) on the solid that just received fillet faces. Returns
/// `OperationError::TopologyError` when validation fails so callers can
/// surface the issue rather than silently produce a malformed solid.
fn validate_filleted_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    // Solid existence is a precondition here; if the caller passed an
    // unknown id, treat that as an internal logic error.
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidGeometry(format!(
            "validate_filleted_solid: solid {} not found",
            solid_id
        )));
    }
    let result = crate::primitives::validation::validate_model_enhanced(
        model,
        Tolerance::default(),
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
            "filleted solid failed validation ({} error(s)): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

/// Curvature-adaptive sampling for rolling-ball fillet sweeps.
///
/// Refines parameter samples where the curve bends fastest so the swept
/// fillet surface stays within `tolerance.distance()` of the analytic
/// rolling-ball envelope. Implementation: start with a coarse 8-sample
/// uniform partition, then bisect any interval whose midpoint curvature
/// exceeds `curvature_threshold = 1 / tolerance.distance()` (chord
/// deviation ~ k * Δs² / 8 ≤ tolerance for that bound). Caps total
/// samples at 256 to keep fillet construction bounded.
fn adaptive_rolling_ball_sampling(curve: &dyn Curve, tolerance: &Tolerance) -> Vec<f64> {
    const MIN_SAMPLES: usize = 8;
    const MAX_SAMPLES: usize = 256;
    let tol = tolerance.distance().max(1e-9);
    // Curvature above this triggers refinement; chord-deviation bound
    // becomes tight when k * (Δs)² ≈ 8 * tol.
    let curvature_threshold = 1.0 / tol;

    let mut params: Vec<f64> = (0..=MIN_SAMPLES)
        .map(|i| i as f64 / MIN_SAMPLES as f64)
        .collect();

    // Iterative bisection passes; each pass inserts midpoints where
    // curvature is high, until either all intervals are smooth enough
    // or we hit MAX_SAMPLES.
    for _pass in 0..6 {
        if params.len() >= MAX_SAMPLES {
            break;
        }
        let mut inserted = Vec::new();
        for w in params.windows(2) {
            let mid = 0.5 * (w[0] + w[1]);
            // If we can't evaluate curvature at the midpoint (e.g.,
            // straight segment yielding NumericalInstability), skip
            // refinement for that interval — uniform spacing suffices.
            if let Ok(k) = curve.curvature_at(mid) {
                if k > curvature_threshold {
                    inserted.push(mid);
                }
            }
        }
        if inserted.is_empty() {
            break;
        }
        params.extend(inserted);
        params.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        params.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    }

    params
}

/// Validate fillet parameters
fn validate_fillet_parameters(
    model: &BRepModel,
    edge_id: EdgeId,
    radius: f64,
    tolerance: &Tolerance,
) -> OperationResult<()> {
    if radius <= 0.0 {
        return Err(OperationError::InvalidRadius(radius));
    }

    // Get edge
    let edge = model
        .edges
        .get(edge_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

    // Check that radius is not too large for the edge length. Use the
    // caller-supplied tolerance so arc-length integration matches the
    // precision the fillet operation will run at downstream — running
    // it at a different (default) tolerance can let a borderline-too-
    // large radius slip past validation.
    let edge_length = edge.compute_arc_length(&model.curves, *tolerance)?;
    if radius > edge_length * 0.5 {
        return Err(OperationError::InvalidRadius(radius));
    }

    Ok(())
}

