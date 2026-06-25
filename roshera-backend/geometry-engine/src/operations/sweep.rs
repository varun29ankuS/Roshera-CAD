//! General Sweep Operations for B-Rep Models
//!
//! Creates solids by sweeping profiles along arbitrary paths with
//! orientation control and scaling.
//!
//! Indexed access into profile-vertex / path-frame arrays is the canonical
//! idiom — all `arr[i]` sites use indices bounded by profile vertex count
//! and path-frame count established at sweep entry. Matches the numerical-
//! kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::lifecycle::{self, OpSpec};
use super::orientation::orient_face_for_outward;
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
    // F2-δ pre-flight: profile + path edges exist.
    if options.common.validate_before {
        let path_slice = [path];
        lifecycle::validate_can_apply(
            model,
            OpSpec::SweepProfile {
                profile_edges: &profile,
                path_edges: &path_slice,
            },
        )?;
    }

    lifecycle::with_rollback(model, move |model| {
        sweep_profile_body(model, profile, path, options)
    })
}

fn sweep_profile_body(
    model: &mut BRepModel,
    profile: Vec<EdgeId>,
    path: EdgeId,
    options: SweepOptions,
) -> OperationResult<SolidId> {
    // Validate inputs
    validate_sweep_inputs(model, &profile, path, &options)?;

    // Capture profile edges before they're consumed, for recording.
    let profile_edges_for_record: Vec<u32> = profile.clone();

    // Get path curve up-front so we can compute the path tangent at
    // t = 0 and pass `-tangent` as the start-cap outward target. The
    // sweep's start cap must point *opposite* the sweep direction
    // (away from the body of the swept solid), and the end cap —
    // synthesised later by `create_reversed_face` — must point along
    // `+tangent_at_end`; flipping a correctly-oriented start cap is
    // exactly what `create_reversed_face` does, so getting the start
    // cap right propagates to the end cap by construction.
    let path_edge = model
        .edges
        .get(path)
        .ok_or_else(|| OperationError::InvalidGeometry("Path edge not found".to_string()))?
        .clone();
    let start_outward_target = sweep_start_cap_outward(model, &path_edge)?;

    // Create face from profile
    let profile_face = create_profile_face(model, profile, start_outward_target)?;

    // Create swept solid based on sweep type
    let solid_id = match options.sweep_type {
        SweepType::Path => create_path_sweep(model, profile_face, &path_edge, &options)?,
        SweepType::MultiGuide => {
            create_frame_driven_sweep(model, profile_face, &path_edge, &options)?
        }
        SweepType::Rail => create_frame_driven_sweep(model, profile_face, &path_edge, &options)?,
        SweepType::BiRail => create_frame_driven_sweep(model, profile_face, &path_edge, &options)?,
    };

    // Drop the scratch profile face. `profile_face` is only a TEMPLATE: every
    // sweep section (start cap, lateral rings, end cap) is built from a fresh
    // `transform_face_full` copy with its own vertices/edges/loop, so the
    // original profile face — and its profile edges — are never part of the
    // result solid's shell. Left in the model they remain single-use faces, and
    // the whole-model validator (`validate_model_enhanced`, which
    // `validate_solid_scoped` runs and whose model-global, unattributed gap
    // errors are NOT filtered out by solid id) flags each profile edge as a
    // boundary-edge "gap in topology" — making a perfectly watertight straight
    // prism report `brep_valid = false`. Removing the scratch face (and its
    // edges not shared with the solid shell) clears those phantom gaps while
    // leaving the clean swept mesh untouched. Mirrors loft's
    // `remove_scratch_profile_faces`.
    remove_scratch_profile_face(model, profile_face, solid_id);

    // Validate result if requested
    if options.common.validate_result {
        validate_swept_solid(model, solid_id)?;
    }

    // Record for attached recorders. Include the sweep type discriminant
    // by Debug-formatting since SweepOptions is not Serialize.
    model.set_solid_provenance(
        solid_id,
        crate::primitives::provenance::OperationKind::Sweep,
        Vec::new(),
    );
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("sweep_profile")
            .with_parameters(serde_json::json!({
                "path_edge": path,
                "sweep_type": format!("{:?}", options.sweep_type),
                "quality": format!("{:?}", options.quality),
            }))
            .with_input_edges(
                profile_edges_for_record
                    .iter()
                    .map(|&e| e as u64)
                    .chain(std::iter::once(path as u64)),
            )
            .with_output_solids([solid_id as u64]),
    );

    Ok(solid_id)
}

/// Create a path sweep
#[allow(clippy::expect_used)] // sections non-empty: generate_sweep_sections returns ≥1 section
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
#[allow(clippy::expect_used)] // sections non-empty: generate_sweep_sections returns ≥1 section
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

    // Guard: an empty `sections` would make `sections[0]` panic and
    // `0..sections.len() - 1` underflow to a near-infinite loop.
    if sections.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "frame-driven sweep produced no sections".to_string(),
        ));
    }

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

/// Compute a world-up oriented frame: tangent from the path, "up" derived
/// from the global +Z axis (or +Y if the tangent is near-parallel to +Z),
/// "side" = tangent × up, then re-orthonormalize. This avoids the
/// curvature-flip artifacts of Frenet on planar paths and gives a
/// predictable orientation aligned with the world reference frame, which
/// is what `OrientationControl::Normal` denotes when no guide surface is
/// supplied.
fn compute_normal_frame(model: &BRepModel, edge: &Edge, t: f64) -> OperationResult<Matrix4> {
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

    // Pick reference up; switch to Y if tangent is near-parallel to Z
    let z = Vector3::new(0.0, 0.0, 1.0);
    let reference = if tangent.dot(&z).abs() > 0.95 {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        z
    };

    // Project reference onto plane perpendicular to tangent
    let up = (reference - tangent * tangent.dot(&reference)).normalize()?;
    let side = tangent.cross(&up).normalize()?;

    Ok(Matrix4::from_cols(side, up, tangent, Vector3::ZERO))
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

    // Get an ordered ring of vertices for lateral-face construction.
    // Closed-curve edges (e.g. a circle expressed as a single self-closing
    // edge) are densified here so consecutive sections produce non-degenerate
    // quads. The transformed face itself stays analytical for caps.
    let vertices = get_section_vertex_ring(model, transformed_face, 32)?;

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

/// Build a polygonal vertex ring around the outer loop of a section face.
///
/// For each edge in the loop, samples the underlying curve at evenly-spaced
/// parameters and creates real vertices in the model. Straight edges
/// contribute exactly their start vertex; closed-curve edges (start == end,
/// e.g. a circular profile expressed as a single self-closing edge) are
/// subdivided into `samples_per_closed_edge` distinct points so that
/// consecutive sweep sections can be stitched with non-degenerate quads.
///
/// The returned ring is in CCW order for a forward-oriented loop and
/// excludes the seam-duplicate so successive entries are distinct.
fn get_section_vertex_ring(
    model: &mut BRepModel,
    face_id: FaceId,
    samples_per_closed_edge: usize,
) -> OperationResult<Vec<VertexId>> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?;
    let outer_loop_id = face.outer_loop;

    let loop_data = model
        .loops
        .get(outer_loop_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut ring: Vec<VertexId> = Vec::new();

    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let forward = loop_data.orientations[i];
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?
            .clone();

        // Detect closed-curve edges by either coincident vertex IDs or
        // coincident endpoint positions (transform_edge re-emits distinct
        // VertexIds for a self-closing source edge, so the ID check alone
        // misses transformed circles/ellipses).
        let curve = model
            .curves
            .get(edge.curve_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
        let lo = edge.param_range.start;
        let hi = edge.param_range.end;
        let endpoints_coincide = if edge.start_vertex == edge.end_vertex {
            true
        } else {
            match (curve.evaluate(lo), curve.evaluate(hi)) {
                (Ok(a), Ok(b)) => (a.position - b.position).magnitude_squared() < 1e-12,
                _ => false,
            }
        };

        if endpoints_coincide {
            // Closed-curve edge: discretize the curve into N sample vertices.
            let n = samples_per_closed_edge.max(3);
            let mut sample_positions: Vec<Point3> = Vec::with_capacity(n);
            for k in 0..n {
                let frac = k as f64 / n as f64;
                let u = lo + (hi - lo) * frac;
                let cp = curve.evaluate(u).map_err(|e| {
                    OperationError::NumericalError(format!(
                        "Curve evaluation failed during section ring discretization: {:?}",
                        e
                    ))
                })?;
                sample_positions.push(cp.position);
            }
            let mut samples: Vec<VertexId> = sample_positions
                .into_iter()
                .map(|p| model.vertices.add(p.x, p.y, p.z))
                .collect();
            if !forward {
                samples.reverse();
            }
            for v in samples {
                if ring.last() != Some(&v) {
                    ring.push(v);
                }
            }
        } else {
            let v = if forward {
                edge.start_vertex
            } else {
                edge.end_vertex
            };
            if ring.last() != Some(&v) {
                ring.push(v);
            }
        }
    }

    // Drop a trailing seam duplicate if the ring closed onto itself.
    if ring.len() > 1 && ring.first() == ring.last() {
        ring.pop();
    }

    if ring.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Section vertex ring needs at least 3 distinct points".to_string(),
        ));
    }

    Ok(ring)
}

/// Create lateral faces between sections.
///
/// Each quad's outward target is the radial direction from the
/// sweep-axis midpoint (mean of the two section centroids) to the
/// quad's own centroid. This places the oriented normal away from
/// the swept solid's interior, satisfying the kernel-wide outward-
/// normal invariant maintained by `orient_face_for_outward`. Sections
/// that collapse to a point (zero-radius profile, degenerate sweep)
/// fall back to the quad's geometric normal so the orientation pick
/// remains deterministic.
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

    // Compute the section centroids and the midline between them. The
    // per-quad radial direction is anchored to this midline so every
    // lateral face's outward target points away from the swept solid.
    let centroid1 = section_centroid(model, section1)?;
    let centroid2 = section_centroid(model, section2)?;
    let axis_mid = (centroid1 + centroid2) * 0.5;

    for i in 0..n {
        let v1 = section1.vertices[i];
        let v2 = section1.vertices[(i + 1) % n];
        let v3 = section2.vertices[(i + 1) % n];
        let v4 = section2.vertices[i];

        let p1 = Vector3::from(
            model
                .vertices
                .get(v1)
                .ok_or_else(|| OperationError::InvalidGeometry("v1 not found".to_string()))?
                .position,
        );
        let p2 = Vector3::from(
            model
                .vertices
                .get(v2)
                .ok_or_else(|| OperationError::InvalidGeometry("v2 not found".to_string()))?
                .position,
        );
        let p3 = Vector3::from(
            model
                .vertices
                .get(v3)
                .ok_or_else(|| OperationError::InvalidGeometry("v3 not found".to_string()))?
                .position,
        );
        let p4 = Vector3::from(
            model
                .vertices
                .get(v4)
                .ok_or_else(|| OperationError::InvalidGeometry("v4 not found".to_string()))?
                .position,
        );
        let quad_centroid = (p1 + p2 + p3 + p4) * 0.25;
        let radial = quad_centroid - axis_mid;
        // Geometric fallback normal — the bilinear-patch's mid-uv normal.
        let fallback = (p2 - p1).cross(&(p4 - p1));
        let outward_target = if radial.magnitude_squared() > 1e-20 {
            radial
        } else if fallback.magnitude_squared() > 1e-20 {
            fallback
        } else {
            Vector3::Z
        };

        let face = create_quad_face(model, v1, v2, v3, v4, outward_target)?;
        faces.push(face);
    }

    Ok(faces)
}

/// Mean position of all vertices in a sweep section.
fn section_centroid(model: &BRepModel, section: &SweepSection) -> OperationResult<Vector3> {
    if section.vertices.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "Sweep section has no vertices".to_string(),
        ));
    }
    let mut sum = Vector3::ZERO;
    for &vid in &section.vertices {
        let v = model.vertices.get(vid).ok_or_else(|| {
            OperationError::InvalidGeometry("Section vertex not found".to_string())
        })?;
        sum = sum + Vector3::from(v.position);
    }
    Ok(sum * (1.0 / section.vertices.len() as f64))
}

/// Create a quadrilateral face whose oriented outward normal aligns
/// with `outward_target`.
///
/// The caller is responsible for computing a meaningful outward
/// direction (typically the radial vector from the sweep midline to
/// the quad centroid). The orientation pick uses
/// `orient_face_for_outward` against the bilinear-patch's parametric-
/// midpoint normal.
fn create_quad_face(
    model: &mut BRepModel,
    v1: VertexId,
    v2: VertexId,
    v3: VertexId,
    v4: VertexId,
    outward_target: Vector3,
) -> OperationResult<FaceId> {
    // Create edges
    let e1 = create_or_find_edge(model, v1, v2)?;
    let e2 = create_or_find_edge(model, v2, v3)?;
    let e3 = create_or_find_edge(model, v3, v4)?;
    let e4 = create_or_find_edge(model, v4, v1)?;

    // Create loop walking v1→v2→v3→v4→v1. Each edge may be a SHARED edge
    // recovered (by `create_or_find_edge`) in either stored direction, so the
    // per-edge loop-forward flag must be derived from the edge's actual
    // start_vertex — hardcoding `true` left the loop a non-closed chain whenever
    // a shared seam edge happened to be stored reversed (#64).
    let mut quad_loop = Loop::new(
        0, // ID will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    for (edge_id, from) in [(e1, v1), (e2, v2), (e3, v3), (e4, v4)] {
        let fwd = model
            .edges
            .get(edge_id)
            .map(|e| e.start_vertex == from)
            .unwrap_or(true);
        quad_loop.add_edge(edge_id, fwd);
    }
    let loop_id = model.loops.add(quad_loop);

    // Create surface (bilinear patch)
    let surface = create_bilinear_surface(model, v1, v2, v3, v4)?;
    let orientation = orient_face_for_outward(surface.as_ref(), outward_target)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        orientation,
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

    // FIND first: reuse an existing edge joining these two vertices (in either
    // direction) so adjacent sweep faces SHARE their seam instead of each
    // minting a coincident duplicate. Without this the function only ever
    // created, so every lateral panel and cap built its own edges and the swept
    // B-Rep was a pile of unstitched, coincident-edged sections that only
    // tessellated watertight by position-welding (SWEEP-BREP-UNSTITCHED #64).
    // The sweep's vertices are shared by id across consecutive sections, so an
    // id match recovers the genuine shared seam (the section ring edge and the
    // neighbouring panel's edge).
    for (eid, edge) in model.edges.iter() {
        if (edge.start_vertex == start && edge.end_vertex == end)
            || (edge.start_vertex == end && edge.end_vertex == start)
        {
            return Ok(eid);
        }
    }

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

/// Create a bilinear (ruled) surface through four corner vertices.
///
/// Models the lateral panel between two consecutive sweep sections as a
/// ruled surface S(u, v) = (1 - v) · L_bottom(u) + v · L_top(u), where
/// L_bottom is the line v1→v2 and L_top is the line v4→v3. This matches the
/// loop traversal v1 → v2 → v3 → v4 used in `create_quad_face`, so the
/// surface's parametric domain coincides with the face's outer loop.
fn create_bilinear_surface(
    model: &BRepModel,
    v1: VertexId,
    v2: VertexId,
    v3: VertexId,
    v4: VertexId,
) -> OperationResult<Box<dyn Surface>> {
    use crate::primitives::curve::Line;
    use crate::primitives::surface::RuledSurface;

    let p1 = Point3::from(
        model
            .vertices
            .get(v1)
            .ok_or_else(|| OperationError::InvalidGeometry("v1 not found".to_string()))?
            .position,
    );
    let p2 = Point3::from(
        model
            .vertices
            .get(v2)
            .ok_or_else(|| OperationError::InvalidGeometry("v2 not found".to_string()))?
            .position,
    );
    let p3 = Point3::from(
        model
            .vertices
            .get(v3)
            .ok_or_else(|| OperationError::InvalidGeometry("v3 not found".to_string()))?
            .position,
    );
    let p4 = Point3::from(
        model
            .vertices
            .get(v4)
            .ok_or_else(|| OperationError::InvalidGeometry("v4 not found".to_string()))?
            .position,
    );

    let bottom = Box::new(Line::new(p1, p2));
    let top = Box::new(Line::new(p4, p3));
    Ok(Box::new(RuledSurface::new(bottom, top)))
}

/// Compute the outward-target direction for the start cap of a sweep.
///
/// Geometrically, the start cap is the back face of the swept solid —
/// its oriented normal must point *away* from the sweep direction.
/// We sample the path's unit tangent at t = 0 and return its negation.
/// A degenerate path (zero-length tangent) is rejected because the
/// sweep cannot proceed.
fn sweep_start_cap_outward(model: &BRepModel, path_edge: &Edge) -> OperationResult<Vector3> {
    let curve = model
        .curves
        .get(path_edge.curve_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Path curve not found".to_string()))?;
    let curve_t = path_edge.edge_to_curve_parameter(0.0);
    let derivs = curve.evaluate_derivatives(curve_t, 1)?;
    let tangent = derivs
        .get(1)
        .ok_or_else(|| OperationError::InvalidGeometry("Path tangent unavailable".to_string()))?;
    if tangent.magnitude_squared() < 1e-20 {
        return Err(OperationError::InvalidGeometry(
            "Path tangent at start is degenerate".to_string(),
        ));
    }
    Ok(-(tangent.normalize()?))
}

/// Create profile face from edges.
///
/// Constructs a planar surface fitted to the profile's actual vertex
/// positions rather than defaulting to the XY plane — sweeping a circle
/// in the YZ plane (e.g. profile normal = +X) would otherwise collapse to
/// a degenerate strip when projected onto Z = 0.
///
/// `outward_target` is the geometric direction the face's oriented
/// outward normal must align with. For a sweep start cap this is
/// `-path_tangent_at_start`.
fn create_profile_face(
    model: &mut BRepModel,
    edges: Vec<EdgeId>,
    outward_target: Vector3,
) -> OperationResult<FaceId> {
    use crate::primitives::surface::Plane;

    // Collect sample points from each edge in loop order. For straight-edge
    // profiles the edge endpoints suffice; closed-curve profiles (e.g. a
    // circle expressed as a single self-closing edge) provide too few
    // distinct vertices, so we additionally sample the underlying curve at
    // its quarter parameters. Three non-collinear samples define the plane.
    let mut points: Vec<Point3> = Vec::with_capacity(edges.len() * 3);
    for &edge_id in &edges {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry("Edge not found".to_string()))?;

        let v_start = model
            .vertices
            .get(edge.start_vertex)
            .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
        points.push(Point3::from(v_start.position));

        // Sample mid-curve and quarter points to capture closed/curved edges
        // whose start and end vertices coincide.
        if edge.start_vertex == edge.end_vertex {
            let curve = model
                .curves
                .get(edge.curve_id)
                .ok_or_else(|| OperationError::InvalidGeometry("Curve not found".to_string()))?;
            let lo = edge.param_range.start;
            let hi = edge.param_range.end;
            for frac in [0.25, 0.5, 0.75] {
                let u = lo + (hi - lo) * frac;
                if let Ok(cp) = curve.evaluate(u) {
                    points.push(cp.position);
                }
            }
        }
    }

    if points.len() < 3 {
        return Err(OperationError::InvalidGeometry(
            "Profile needs at least three sample points to define a plane".to_string(),
        ));
    }

    // Pick three non-collinear points: p0 fixed, scan for first p_i with a
    // sufficiently long edge p0→p_i, then for first p_j with a non-trivial
    // cross product against (p_i - p0).
    let p0 = points[0];
    let tol_sq = 1e-20;
    let mut p_i = None;
    let mut idx_i = 0;
    for (k, p) in points.iter().enumerate().skip(1) {
        if (*p - p0).magnitude_squared() > tol_sq {
            p_i = Some(*p);
            idx_i = k;
            break;
        }
    }
    let p_i = p_i.ok_or_else(|| {
        OperationError::InvalidGeometry("Profile vertices are coincident".to_string())
    })?;

    let v_i = p_i - p0;
    let mut p_j = None;
    for (k, p) in points.iter().enumerate().skip(idx_i + 1) {
        if (*p - p0).magnitude_squared() < tol_sq {
            continue;
        }
        let v_k = *p - p0;
        if v_i.cross(&v_k).magnitude_squared() > tol_sq {
            p_j = Some(points[k]);
            break;
        }
    }
    let p_j = p_j.ok_or_else(|| {
        OperationError::InvalidGeometry("Profile vertices are collinear".to_string())
    })?;

    let plane = Plane::from_three_points(p0, p_i, p_j).map_err(|e| {
        OperationError::NumericalError(format!("Profile plane fit failed: {:?}", e))
    })?;

    // Create loop from edges
    let mut profile_loop = Loop::new(
        0, // ID will be assigned by store
        crate::primitives::r#loop::LoopType::Outer,
    );
    for edge_id in edges {
        profile_loop.add_edge(edge_id, true);
    }
    let loop_id = model.loops.add(profile_loop);

    let surface: Box<dyn Surface> = Box::new(plane);
    let orientation = orient_face_for_outward(surface.as_ref(), outward_target)?;
    let surface_id = model.surfaces.add(surface);

    // Create face
    let face = Face::new(
        0, // ID will be assigned by store
        surface_id,
        loop_id,
        orientation,
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

/// Remove the scratch profile face (the sweep TEMPLATE) from the model after
/// the result solid is built.
///
/// `create_profile_face` builds a planar face from the caller's profile edges
/// purely so each sweep section can be produced as a transformed COPY of it
/// (`transform_face_full` → fresh surface/loop/edges/vertices). The original
/// face is therefore never wired into the result solid's shell. Left behind it
/// is a single-use face whose profile edges the whole-model validator reports
/// as boundary-edge gaps (those gap errors carry no solid id, so
/// `validate_solid_scoped` cannot filter them out), spuriously failing the
/// B-Rep validity of an otherwise-watertight prism.
///
/// Each profile edge is removed only when it is NOT also referenced by the
/// result solid's shell, so a sweep variant that happened to share an input
/// edge with a lateral/cap face never loses a live edge. Directly mirrors
/// loft's `remove_scratch_profile_faces`.
fn remove_scratch_profile_face(model: &mut BRepModel, profile_face: FaceId, solid_id: SolidId) {
    use std::collections::HashSet;

    // Edges referenced by the result solid's shell faces (outer + inner shells).
    let mut solid_edges: HashSet<EdgeId> = HashSet::new();
    if let Some(solid) = model.solids.get(solid_id) {
        let shell_ids: Vec<_> = std::iter::once(solid.outer_shell)
            .chain(solid.inner_shells.iter().copied())
            .collect();
        for shid in shell_ids {
            let shell_faces: Vec<FaceId> = model
                .shells
                .get(shid)
                .map(|s| s.faces.clone())
                .unwrap_or_default();
            for fid in shell_faces {
                if let Some(face) = model.faces.get(fid) {
                    let loops =
                        std::iter::once(face.outer_loop).chain(face.inner_loops.iter().copied());
                    for lid in loops {
                        if let Some(lp) = model.loops.get(lid) {
                            for &e in &lp.edges {
                                solid_edges.insert(e);
                            }
                        }
                    }
                }
            }
        }
    }

    let (outer, inner) = match model.faces.get(profile_face) {
        Some(f) => (f.outer_loop, f.inner_loops.clone()),
        None => return,
    };
    for lid in std::iter::once(outer).chain(inner) {
        if let Some(lp) = model.loops.get(lid).cloned() {
            for e in lp.edges {
                if !solid_edges.contains(&e) {
                    model.edges.remove(e);
                }
            }
        }
        model.loops.remove(lid);
    }
    model.faces.remove(profile_face);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::curve::Line;
    use crate::primitives::topology_builder::BRepModel;

    /// Add a Line curve + Edge between two existing vertices.
    fn add_line_edge(model: &mut BRepModel, v_start: VertexId, v_end: VertexId) -> EdgeId {
        let s = model.vertices.get(v_start).expect("start vertex");
        let e = model.vertices.get(v_end).expect("end vertex");
        let line = Line::new(Point3::from(s.position), Point3::from(e.position));
        let curve_id = model.curves.add(Box::new(line));
        let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
        model.edges.add(edge)
    }

    /// Closed CCW unit-square profile in the XY plane.
    fn make_unit_square(model: &mut BRepModel) -> Vec<EdgeId> {
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(1.0, 1.0, 0.0);
        let v3 = model.vertices.add(0.0, 1.0, 0.0);
        vec![
            add_line_edge(model, v0, v1),
            add_line_edge(model, v1, v2),
            add_line_edge(model, v2, v3),
            add_line_edge(model, v3, v0),
        ]
    }

    // -------------------------------------------------------------------
    // SweepOptions defaults & Debug impls
    // -------------------------------------------------------------------

    #[test]
    fn sweep_options_default_values() {
        let opts = SweepOptions::default();
        assert_eq!(opts.sweep_type, SweepType::Path);
        assert!(matches!(opts.orientation, OrientationControl::Frenet));
        assert!(matches!(opts.scale, ScaleControl::Constant));
        assert!(matches!(opts.twist, TwistControl::None));
        assert!(opts.create_solid);
        assert_eq!(opts.quality, SweepQuality::Standard);
    }

    #[test]
    fn sweep_type_variants_distinct() {
        assert_ne!(SweepType::Path, SweepType::MultiGuide);
        assert_ne!(SweepType::Rail, SweepType::BiRail);
        assert_eq!(SweepType::Path, SweepType::Path);
    }

    #[test]
    fn sweep_quality_variants_distinct() {
        assert_ne!(SweepQuality::Draft, SweepQuality::Standard);
        assert_ne!(SweepQuality::Standard, SweepQuality::High);
    }

    #[test]
    fn orientation_control_debug_covers_each_variant() {
        let frenet = format!("{:?}", OrientationControl::Frenet);
        assert!(frenet.contains("Frenet"));
        let minimal = format!("{:?}", OrientationControl::MinimalRotation);
        assert!(minimal.contains("MinimalRotation"));
        let fixed = format!("{:?}", OrientationControl::Fixed(Vector3::Z));
        assert!(fixed.contains("Fixed"));
        let normal = format!("{:?}", OrientationControl::Normal);
        assert!(normal.contains("Normal"));
        let custom = format!(
            "{:?}",
            OrientationControl::Custom(Box::new(|_| Matrix4::identity()))
        );
        assert!(custom.contains("Custom"));
    }

    #[test]
    fn scale_control_debug_covers_each_variant() {
        let constant = format!("{:?}", ScaleControl::Constant);
        assert!(constant.contains("Constant"));
        let linear = format!("{:?}", ScaleControl::Linear(0.5, 1.5));
        assert!(linear.contains("Linear"));
        let function = format!("{:?}", ScaleControl::Function(Box::new(|t| t)));
        assert!(function.contains("Function"));
    }

    #[test]
    fn twist_control_debug_covers_each_variant() {
        let none = format!("{:?}", TwistControl::None);
        assert!(none.contains("None"));
        let linear = format!("{:?}", TwistControl::Linear(1.0));
        assert!(linear.contains("Linear"));
        let function = format!("{:?}", TwistControl::Function(Box::new(|t| t)));
        assert!(function.contains("Function"));
    }

    // -------------------------------------------------------------------
    // compute_scale_at_parameter / compute_twist_at_parameter
    // -------------------------------------------------------------------

    #[test]
    fn compute_scale_constant_returns_unit() {
        let v = compute_scale_at_parameter(0.42, &ScaleControl::Constant).expect("ok");
        assert!((v - 1.0).abs() < 1e-12);
    }

    #[test]
    fn compute_scale_linear_interpolates_endpoints() {
        let ctrl = ScaleControl::Linear(0.5, 1.5);
        assert!((compute_scale_at_parameter(0.0, &ctrl).expect("ok") - 0.5).abs() < 1e-12);
        assert!((compute_scale_at_parameter(1.0, &ctrl).expect("ok") - 1.5).abs() < 1e-12);
        assert!((compute_scale_at_parameter(0.5, &ctrl).expect("ok") - 1.0).abs() < 1e-12);
    }

    #[test]
    fn compute_scale_function_invokes_closure() {
        let ctrl = ScaleControl::Function(Box::new(|t| 2.0 * t + 0.25));
        let v = compute_scale_at_parameter(0.5, &ctrl).expect("ok");
        assert!((v - 1.25).abs() < 1e-12);
    }

    #[test]
    fn compute_twist_none_returns_zero() {
        let v = compute_twist_at_parameter(0.7, &TwistControl::None).expect("ok");
        assert_eq!(v, 0.0);
    }

    #[test]
    fn compute_twist_linear_scales_with_t() {
        let ctrl = TwistControl::Linear(std::f64::consts::PI);
        let v = compute_twist_at_parameter(0.5, &ctrl).expect("ok");
        assert!((v - std::f64::consts::FRAC_PI_2).abs() < 1e-12);
    }

    #[test]
    fn compute_twist_function_invokes_closure() {
        let ctrl = TwistControl::Function(Box::new(|t| t * t));
        let v = compute_twist_at_parameter(0.4, &ctrl).expect("ok");
        assert!((v - 0.16).abs() < 1e-12);
    }

    // -------------------------------------------------------------------
    // build_sweep_transform
    // -------------------------------------------------------------------

    #[test]
    fn build_sweep_transform_zero_twist_unit_scale_is_translation() {
        let m = build_sweep_transform(Point3::new(3.0, 4.0, 5.0), Matrix4::identity(), 1.0, 0.0);
        let p = m.transform_point(&Vector3::ZERO);
        assert!((p.x - 3.0).abs() < 1e-12);
        assert!((p.y - 4.0).abs() < 1e-12);
        assert!((p.z - 5.0).abs() < 1e-12);
    }

    // -------------------------------------------------------------------
    // validate_sweep_inputs / validate_swept_solid
    // -------------------------------------------------------------------

    #[test]
    fn validate_sweep_inputs_rejects_unknown_profile_edge() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(0.0, 0.0, 1.0);
        let path = add_line_edge(&mut model, v0, v1);
        let result = validate_sweep_inputs(&model, &[9999], path, &SweepOptions::default());
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_sweep_inputs_rejects_unknown_path() {
        let mut model = BRepModel::new();
        let edges = make_unit_square(&mut model);
        let result = validate_sweep_inputs(&model, &edges, 9999, &SweepOptions::default());
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn validate_sweep_inputs_accepts_valid_inputs() {
        let mut model = BRepModel::new();
        let profile = make_unit_square(&mut model);
        let v_a = model.vertices.add(0.0, 0.0, 0.0);
        let v_b = model.vertices.add(0.0, 0.0, 5.0);
        let path = add_line_edge(&mut model, v_a, v_b);
        assert!(validate_sweep_inputs(&model, &profile, path, &SweepOptions::default()).is_ok());
    }

    #[test]
    fn validate_swept_solid_rejects_unknown_solid() {
        let model = BRepModel::new();
        let result = validate_swept_solid(&model, 9999);
        assert!(matches!(result, Err(OperationError::InvalidBRep(_))));
    }

    // -------------------------------------------------------------------
    // create_or_find_edge / create_bilinear_surface / create_quad_face
    // -------------------------------------------------------------------

    #[test]
    fn create_or_find_edge_links_existing_vertices() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let edge_id = create_or_find_edge(&mut model, v0, v1).expect("edge");
        let edge = model.edges.get(edge_id).expect("edge in store");
        assert_eq!(edge.start_vertex, v0);
        assert_eq!(edge.end_vertex, v1);
    }

    #[test]
    fn create_or_find_edge_rejects_unknown_start_vertex() {
        let mut model = BRepModel::new();
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let result = create_or_find_edge(&mut model, 9999, v1);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn create_or_find_edge_rejects_unknown_end_vertex() {
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let result = create_or_find_edge(&mut model, v0, 9999);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn create_bilinear_surface_returns_surface_for_quad() {
        let mut model = BRepModel::new();
        let v1 = model.vertices.add(0.0, 0.0, 0.0);
        let v2 = model.vertices.add(1.0, 0.0, 0.0);
        let v3 = model.vertices.add(1.0, 1.0, 0.0);
        let v4 = model.vertices.add(0.0, 1.0, 0.0);
        assert!(create_bilinear_surface(&model, v1, v2, v3, v4).is_ok());
    }

    #[test]
    fn create_bilinear_surface_rejects_unknown_vertex() {
        let model = BRepModel::new();
        let result = create_bilinear_surface(&model, 1, 2, 3, 4);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn create_quad_face_produces_face_with_outer_loop_of_4_edges() {
        let mut model = BRepModel::new();
        let v1 = model.vertices.add(0.0, 0.0, 0.0);
        let v2 = model.vertices.add(1.0, 0.0, 0.0);
        let v3 = model.vertices.add(1.0, 1.0, 0.0);
        let v4 = model.vertices.add(0.0, 1.0, 0.0);
        let face_id = create_quad_face(&mut model, v1, v2, v3, v4, Vector3::Z).expect("face");
        let face = model.faces.get(face_id).expect("face in store");
        let outer = model.loops.get(face.outer_loop).expect("loop");
        assert_eq!(outer.edges.len(), 4);
    }

    // -------------------------------------------------------------------
    // create_lateral_faces / create_reversed_face
    // -------------------------------------------------------------------

    #[test]
    fn create_lateral_faces_rejects_mismatched_vertex_counts() {
        let mut model = BRepModel::new();
        let s1 = SweepSection {
            face_id: 0,
            vertices: vec![1, 2, 3],
        };
        let s2 = SweepSection {
            face_id: 0,
            vertices: vec![4, 5],
        };
        let result = create_lateral_faces(&mut model, &s1, &s2);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn create_lateral_faces_creates_n_quads_for_n_vertices() {
        let mut model = BRepModel::new();
        let v_bottom: Vec<VertexId> = (0..4)
            .map(|i| model.vertices.add(i as f64, 0.0, 0.0))
            .collect();
        let v_top: Vec<VertexId> = (0..4)
            .map(|i| model.vertices.add(i as f64, 0.0, 1.0))
            .collect();
        let s1 = SweepSection {
            face_id: 0,
            vertices: v_bottom,
        };
        let s2 = SweepSection {
            face_id: 0,
            vertices: v_top,
        };
        let faces = create_lateral_faces(&mut model, &s1, &s2).expect("ok");
        assert_eq!(faces.len(), 4);
    }

    #[test]
    fn create_reversed_face_flips_orientation() {
        let mut model = BRepModel::new();
        let v1 = model.vertices.add(0.0, 0.0, 0.0);
        let v2 = model.vertices.add(1.0, 0.0, 0.0);
        let v3 = model.vertices.add(1.0, 1.0, 0.0);
        let v4 = model.vertices.add(0.0, 1.0, 0.0);
        let face_id = create_quad_face(&mut model, v1, v2, v3, v4, Vector3::Z).expect("face");
        let original_orientation = model.faces.get(face_id).expect("face").orientation;
        let reversed_id = create_reversed_face(&mut model, face_id).expect("reversed");
        let reversed_orientation = model
            .faces
            .get(reversed_id)
            .expect("reversed face")
            .orientation;
        assert_ne!(original_orientation, reversed_orientation);
    }

    #[test]
    fn create_reversed_face_rejects_unknown_face() {
        let mut model = BRepModel::new();
        let result = create_reversed_face(&mut model, 9999);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    // -------------------------------------------------------------------
    // create_profile_face
    // -------------------------------------------------------------------

    #[test]
    fn create_profile_face_from_rectangle_succeeds() {
        let mut model = BRepModel::new();
        let edges = make_unit_square(&mut model);
        let face_id = create_profile_face(&mut model, edges, Vector3::Z).expect("face");
        assert!(model.faces.get(face_id).is_some());
    }

    #[test]
    fn create_profile_face_rejects_too_few_points() {
        let mut model = BRepModel::new();
        // Single edge → only 1 point gathered (start vertex), insufficient.
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let e0 = add_line_edge(&mut model, v0, v1);
        let result = create_profile_face(&mut model, vec![e0, e0], Vector3::Z);
        // Two edges = 2 distinct start positions only; below the 3-point threshold.
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn create_profile_face_rejects_collinear_profile() {
        // Three edges on the same line — collinear samples, no plane.
        let mut model = BRepModel::new();
        let v0 = model.vertices.add(0.0, 0.0, 0.0);
        let v1 = model.vertices.add(1.0, 0.0, 0.0);
        let v2 = model.vertices.add(2.0, 0.0, 0.0);
        let v3 = model.vertices.add(3.0, 0.0, 0.0);
        let e0 = add_line_edge(&mut model, v0, v1);
        let e1 = add_line_edge(&mut model, v1, v2);
        let e2 = add_line_edge(&mut model, v2, v3);
        let result = create_profile_face(&mut model, vec![e0, e1, e2], Vector3::Z);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn create_profile_face_rejects_unknown_edge() {
        let mut model = BRepModel::new();
        let result = create_profile_face(&mut model, vec![9999], Vector3::Z);
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    // -------------------------------------------------------------------
    // sweep_profile entry point
    // -------------------------------------------------------------------

    #[test]
    fn sweep_profile_validates_unknown_profile_edge() {
        let mut model = BRepModel::new();
        let v_a = model.vertices.add(0.0, 0.0, 0.0);
        let v_b = model.vertices.add(0.0, 0.0, 1.0);
        let path = add_line_edge(&mut model, v_a, v_b);
        let opts = SweepOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = sweep_profile(&mut model, vec![9999], path, opts);
        // F2-δ: pre-flight resolves entity IDs and returns InvalidInput.
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    #[test]
    fn sweep_profile_validates_unknown_path() {
        let mut model = BRepModel::new();
        let edges = make_unit_square(&mut model);
        let opts = SweepOptions {
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let result = sweep_profile(&mut model, edges, 9999, opts);
        // F2-δ: pre-flight resolves entity IDs and returns InvalidInput.
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    #[test]
    fn sweep_profile_along_z_line_creates_solid() {
        let mut model = BRepModel::new();
        let profile = make_unit_square(&mut model);
        let v_a = model.vertices.add(0.0, 0.0, 0.0);
        let v_b = model.vertices.add(0.0, 0.0, 5.0);
        let path = add_line_edge(&mut model, v_a, v_b);
        let opts = SweepOptions {
            quality: SweepQuality::Draft,
            common: CommonOptions {
                validate_result: false,
                ..Default::default()
            },
            ..Default::default()
        };
        let solid_id = sweep_profile(&mut model, profile, path, opts).expect("sweep");
        assert!(model.solids.get(solid_id).is_some());
    }
}
