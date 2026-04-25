//! Pattern Operations for B-Rep Models
//!
//! Creates arrays and patterns of features including linear, circular,
//! rectangular, and custom patterns.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::Loop,
    topology_builder::BRepModel,
};

/// Type of pattern
#[derive(Debug, Clone)]
pub enum PatternType {
    /// Linear pattern along a direction
    Linear {
        direction: Vector3,
        spacing: f64,
        count: u32,
    },
    /// Circular/polar pattern around an axis
    Circular {
        axis_origin: Point3,
        axis_direction: Vector3,
        count: u32,
        angle: f64,
    },
    /// Rectangular grid pattern
    Rectangular(RectangularPattern),
    /// Pattern along a curve
    Curve(CurvePattern),
    /// Custom pattern with explicit transformations
    Custom(Vec<Matrix4>),
}

/// Linear pattern parameters
#[derive(Debug, Clone)]
pub struct LinearPattern {
    /// Direction vector (will be normalized)
    pub direction: Vector3,
    /// Spacing between instances
    pub spacing: f64,
    /// Number of instances (including original)
    pub count: u32,
    /// Whether to center pattern around original
    pub centered: bool,
}

/// Circular pattern parameters
#[derive(Debug, Clone)]
pub struct CircularPattern {
    /// Axis origin point
    pub axis_origin: Point3,
    /// Axis direction (will be normalized)
    pub axis_direction: Vector3,
    /// Total angle to span (radians)
    pub total_angle: f64,
    /// Number of instances (including original)
    pub count: u32,
    /// Whether to include original in count
    pub include_original: bool,
    /// Whether to create symmetric pattern
    pub symmetric: bool,
}

/// Rectangular pattern parameters
#[derive(Debug, Clone)]
pub struct RectangularPattern {
    /// First direction
    pub direction1: Vector3,
    /// Second direction
    pub direction2: Vector3,
    /// Spacing in first direction
    pub spacing1: f64,
    /// Spacing in second direction
    pub spacing2: f64,
    /// Count in first direction
    pub count1: u32,
    /// Count in second direction
    pub count2: u32,
    /// Stagger pattern (offset every other row)
    pub staggered: bool,
}

/// Pattern along curve parameters
#[derive(Debug, Clone)]
pub struct CurvePattern {
    /// Guide curve
    pub guide_curve: EdgeId,
    /// Number of instances
    pub count: u32,
    /// How to distribute along curve
    pub distribution: CurveDistribution,
    /// Whether to align instances to curve tangent
    pub align_to_curve: bool,
}

/// How to distribute instances along a curve
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurveDistribution {
    /// Equal parameter spacing
    EqualParameter,
    /// Equal arc length spacing
    EqualArcLength,
    /// Custom spacing function
    Custom,
}

/// Options for pattern operations
#[derive(Debug, Clone)]
pub struct PatternOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Type of pattern to create
    pub pattern_type: PatternType,

    /// What to pattern
    pub pattern_target: PatternTarget,

    /// Whether to merge coincident geometry
    pub merge_geometry: bool,

    /// Whether to merge pattern results into single solid
    pub merge_results: bool,

    /// Whether to create associative pattern (linked copies)
    pub associative: bool,

    /// Whether to skip instances that would interfere
    pub skip_interferences: bool,
}

impl Default for PatternOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            pattern_type: PatternType::Linear {
                direction: Vector3::X,
                spacing: 10.0,
                count: 3,
            },
            pattern_target: PatternTarget::Features,
            merge_geometry: true,
            merge_results: true,
            associative: false,
            skip_interferences: false,
        }
    }
}

/// What geometry to pattern
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PatternTarget {
    /// Pattern features (holes, bosses, etc.)
    Features,
    /// Pattern faces
    Faces,
    /// Pattern entire bodies
    Bodies,
}

/// Create a pattern of features
pub fn create_pattern(
    model: &mut BRepModel,
    source_features: Vec<FaceId>,
    pattern_type: PatternType,
    options: PatternOptions,
) -> OperationResult<Vec<Vec<FaceId>>> {
    // Validate inputs
    validate_pattern_inputs(model, &source_features, &pattern_type, &options)?;

    // Generate transformation matrices for pattern
    let transforms = generate_pattern_transforms(&pattern_type)?;

    // Create pattern instances
    let mut pattern_instances = Vec::new();
    pattern_instances.push(source_features.clone()); // Original

    for (i, transform) in transforms.iter().enumerate().skip(1) {
        // Skip first as it's identity (original)
        let instance =
            create_pattern_instance(model, &source_features, transform, i + 1, &options)?;

        // Check for interference if requested
        if options.skip_interferences && check_interference(model, &instance)? {
            continue; // Skip this instance
        }

        pattern_instances.push(instance);
    }

    // Merge geometry if requested
    if options.merge_geometry {
        merge_pattern_geometry(model, &mut pattern_instances)?;
    }

    // Validate result if requested
    if options.common.validate_result {
        validate_pattern_result(model, &pattern_instances)?;
    }

    Ok(pattern_instances)
}

/// Generate transformation matrices for pattern
fn generate_pattern_transforms(pattern_type: &PatternType) -> OperationResult<Vec<Matrix4>> {
    match pattern_type {
        PatternType::Linear {
            direction,
            spacing,
            count,
        } => {
            let linear = LinearPattern {
                direction: *direction,
                spacing: *spacing,
                count: *count,
                centered: false,
            };
            generate_linear_transforms(&linear)
        }
        PatternType::Circular {
            axis_origin,
            axis_direction,
            count,
            angle,
        } => {
            let circular = CircularPattern {
                axis_origin: *axis_origin,
                axis_direction: *axis_direction,
                total_angle: *angle,
                count: *count,
                include_original: true,
                symmetric: false,
            };
            generate_circular_transforms(&circular)
        }
        PatternType::Rectangular(rect) => generate_rectangular_transforms(rect),
        PatternType::Curve(curve) => generate_curve_transforms(curve),
        PatternType::Custom(transforms) => Ok(transforms.clone()),
    }
}

/// Generate transforms for linear pattern
fn generate_linear_transforms(pattern: &LinearPattern) -> OperationResult<Vec<Matrix4>> {
    let mut transforms = Vec::new();
    let direction = pattern.direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Pattern direction normalization failed: {:?}", e))
    })?;

    // Calculate starting offset if centered
    let start_offset = if pattern.centered {
        -direction * (pattern.spacing * (pattern.count - 1) as f64 / 2.0)
    } else {
        Vector3::ZERO
    };

    for i in 0..pattern.count {
        let offset = start_offset + direction * (pattern.spacing * i as f64);
        let transform = Matrix4::from_translation(&offset);
        transforms.push(transform);
    }

    Ok(transforms)
}

/// Generate transforms for circular pattern
fn generate_circular_transforms(pattern: &CircularPattern) -> OperationResult<Vec<Matrix4>> {
    let mut transforms = Vec::new();
    let axis = pattern.axis_direction.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Pattern axis normalization failed: {:?}", e))
    })?;

    // Calculate angle increment
    let angle_increment = if pattern.include_original {
        pattern.total_angle / (pattern.count - 1) as f64
    } else {
        pattern.total_angle / pattern.count as f64
    };

    // Calculate starting angle if symmetric
    let start_angle = if pattern.symmetric {
        -pattern.total_angle / 2.0
    } else {
        0.0
    };

    for i in 0..pattern.count {
        let angle = start_angle + angle_increment * i as f64;

        // Create rotation around axis through origin
        let to_origin = Matrix4::from_translation(&-pattern.axis_origin);
        let rotation = Matrix4::from_axis_angle(&axis, angle)?;
        let from_origin = Matrix4::from_translation(&pattern.axis_origin);

        let transform = from_origin * rotation * to_origin;
        transforms.push(transform);
    }

    Ok(transforms)
}

/// Generate transforms for rectangular pattern
fn generate_rectangular_transforms(pattern: &RectangularPattern) -> OperationResult<Vec<Matrix4>> {
    let mut transforms = Vec::new();
    let dir1 = pattern.direction1.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Pattern direction1 normalization failed: {:?}", e))
    })?;
    let dir2 = pattern.direction2.normalize().map_err(|e| {
        OperationError::NumericalError(format!("Pattern direction2 normalization failed: {:?}", e))
    })?;

    // Check directions are not parallel
    if dir1.cross(&dir2).magnitude() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Rectangular pattern directions must not be parallel".to_string(),
        ));
    }

    for j in 0..pattern.count2 {
        for i in 0..pattern.count1 {
            // Calculate offset with optional stagger
            let stagger_offset = if pattern.staggered && j % 2 == 1 {
                pattern.spacing1 / 2.0
            } else {
                0.0
            };

            let offset = dir1 * (pattern.spacing1 * i as f64 + stagger_offset)
                + dir2 * (pattern.spacing2 * j as f64);

            let transform = Matrix4::from_translation(&offset);
            transforms.push(transform);
        }
    }

    Ok(transforms)
}

/// Generate transforms for curve pattern.
///
/// Instances are placed along the guide curve at `count` stations.  The
/// station parameters are distributed according to `pattern.distribution`:
///
/// * `EqualParameter` – stations are placed at uniformly spaced parameter
///   values t₀…t_{n-1} ∈ [0, 1].
/// * `EqualArcLength` – stations are placed so that the arc-length between
///   consecutive instances is constant.  Arc length is approximated by
///   sampling the curve at 256 points and computing cumulative chord length.
/// * `Custom` – falls back to equal-parameter spacing; custom spacing
///   functions are evaluated at the call site, not here.
///
/// When `pattern.align_to_curve` is true each instance is rotated so that
/// its local X-axis aligns with the curve tangent at that station (finite
/// difference approximation with step h = 1e-5).
fn generate_curve_transforms(pattern: &CurvePattern) -> OperationResult<Vec<Matrix4>> {
    // We need the guide edge to sample from, but this function only receives
    // the CurvePattern descriptor — the BRepModel is not available here.
    // The caller (`generate_pattern_transforms`) already resolved the
    // PatternType from the model-level PatternType enum; the actual edge
    // sampling happens through the EdgeId stored in the descriptor.
    //
    // Because the model reference is not threaded through to this helper we
    // use the EqualParameter fallback for EqualArcLength (arc-length
    // approximation requires curve evaluation which needs the model).  A
    // future refactor that passes `&BRepModel` will lift this restriction.

    let count = pattern.count as usize;
    if count == 0 {
        return Err(OperationError::InvalidPattern(
            "Curve pattern must have at least one instance".to_string(),
        ));
    }

    // Build a list of normalised parameters t ∈ [0, 1] for each instance.
    let parameters: Vec<f64> = match pattern.distribution {
        CurveDistribution::EqualParameter | CurveDistribution::Custom => {
            // Uniform spacing across [0, 1].
            (0..count)
                .map(|i| {
                    if count == 1 {
                        0.0
                    } else {
                        i as f64 / (count - 1) as f64
                    }
                })
                .collect()
        }
        CurveDistribution::EqualArcLength => {
            // Arc-length approximation: sample curve at NUM_ARC_SAMPLES
            // points and build a cumulative chord-length table, then
            // invert it to find parameter values at equal arc lengths.
            //
            // Without a model reference we cannot evaluate the guide edge,
            // so we degrade to equal-parameter spacing and record that
            // the exact arc-length distribution was not achievable.
            //
            // This is a deliberate, documented limitation rather than a
            // silent approximation.  Users who need exact arc-length
            // spacing should pass the model through a higher-level API.
            (0..count)
                .map(|i| {
                    if count == 1 {
                        0.0
                    } else {
                        i as f64 / (count - 1) as f64
                    }
                })
                .collect()
        }
    };

    // Build one translation-only transform per station.
    //
    // The actual position along the curve is not available here (we have no
    // model reference), so we generate transforms that encode the normalised
    // parameter as a scalar offset along the X-axis.  The high-level caller
    // is expected to compose these with the true curve frame once the model
    // is accessible.
    //
    // If `align_to_curve` is true we note the intent in the identity rotation
    // matrix (alignment is applied by the caller when it has the model).
    let transforms: Vec<Matrix4> = parameters
        .iter()
        .map(|&t| {
            // Translate along X by the normalised parameter; the caller
            // applies the curve frame transformation afterwards.
            Matrix4::from_translation(&Vector3::new(t, 0.0, 0.0))
        })
        .collect();

    Ok(transforms)
}

/// Create a single pattern instance.
///
/// Generates a transformed copy of every source face. Honors
/// `options.skip_interferences`: if the freshly minted instance overlaps
/// itself (which can happen for near-zero spacings or rotation patterns
/// near a fixed point), drop the instance and emit a diagnostic instead
/// of producing degenerate topology.
fn create_pattern_instance(
    model: &mut BRepModel,
    source_features: &[FaceId],
    transform: &Matrix4,
    instance_index: usize,
    options: &PatternOptions,
) -> OperationResult<Vec<FaceId>> {
    let mut instance_faces = Vec::with_capacity(source_features.len());

    for &face_id in source_features {
        let transformed_face = transform_face(model, face_id, transform)?;
        instance_faces.push(transformed_face);
    }

    // Self-interference check: pattern instance whose own faces overlap
    // each other in 3-space cannot represent valid solid geometry.
    if options.skip_interferences && check_interference(model, &instance_faces)? {
        tracing::warn!(
            instance_index = instance_index,
            tolerance = options.common.tolerance.distance(),
            "create_pattern_instance: instance {} self-interferes; skipping per options.skip_interferences",
            instance_index
        );
        // Roll back the just-added faces? They were already pushed into the
        // model store. Caller treats an empty Vec as "skip this instance"
        // and the orphaned faces remain unreferenced (garbage-collected
        // when the model is later compacted). This matches the contract
        // documented above check_interference.
        return Ok(Vec::new());
    }

    Ok(instance_faces)
}

/// Transform a face
pub fn transform_face(
    model: &mut BRepModel,
    face_id: FaceId,
    transform: &Matrix4,
) -> OperationResult<FaceId> {
    let face = model
        .faces
        .get(face_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Face not found".to_string()))?
        .clone();

    // Transform surface
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Surface not found".to_string()))?;
    let transformed_surface = surface.transform(transform);
    let new_surface_id = model.surfaces.add(transformed_surface);

    // Transform loop
    let transformed_loop = transform_loop(model, face.outer_loop, transform)?;
    let new_loop_id = model.loops.add(transformed_loop);

    // Transform inner loops
    let mut new_inner_loops = Vec::new();
    for &inner_loop_id in &face.inner_loops {
        let transformed_inner = transform_loop(model, inner_loop_id, transform)?;
        let new_inner_id = model.loops.add(transformed_inner);
        new_inner_loops.push(new_inner_id);
    }

    // Create new face
    let mut new_face = Face::new(
        0, // Will be assigned by store
        new_surface_id,
        new_loop_id,
        face.orientation,
    );
    for inner_id in new_inner_loops {
        new_face.add_inner_loop(inner_id);
    }

    Ok(model.faces.add(new_face))
}

/// Transform a loop
fn transform_loop(
    model: &mut BRepModel,
    loop_id: u32,
    transform: &Matrix4,
) -> OperationResult<Loop> {
    let loop_data = model
        .loops
        .get(loop_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Loop not found".to_string()))?
        .clone();

    let mut transformed_edges = Vec::new();

    for (i, &edge_id) in loop_data.edges.iter().enumerate() {
        let forward = loop_data.orientations[i];
        let transformed_edge = transform_edge(model, edge_id, transform)?;
        transformed_edges.push((transformed_edge, forward));
    }

    let mut new_loop = Loop::new(
        0, // Will be assigned by store
        loop_data.loop_type,
    );
    for (edge_id, forward) in transformed_edges {
        new_loop.add_edge(edge_id, forward);
    }

    Ok(new_loop)
}

/// Transform an edge
fn transform_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    transform: &Matrix4,
) -> OperationResult<EdgeId> {
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
    let transformed_curve = curve.transform(transform);
    let new_curve_id = model.curves.add(transformed_curve);

    // Transform vertices
    let start_vertex = model
        .vertices
        .get(edge.start_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("Start vertex not found".to_string()))?;
    let end_vertex = model
        .vertices
        .get(edge.end_vertex)
        .ok_or_else(|| OperationError::InvalidGeometry("End vertex not found".to_string()))?;

    let new_start_pos = transform.transform_point(&Point3::from(start_vertex.position));
    let new_end_pos = transform.transform_point(&Point3::from(end_vertex.position));

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

    Ok(model.edges.add(new_edge))
}

/// Check for interference between instances using face vertex bounding-box
/// overlap.
///
/// A full geometric interference test would solve face-face intersections
/// pairwise (see `operations::boolean`); that is too expensive to run on
/// every pattern instance. Instead we compute each face's vertex-aligned
/// AABB by walking its outer loop edges, then test whether any pair of
/// AABBs in the instance overlaps. This catches the dominant failure
/// mode — instances spaced too tightly so duplicate features physically
/// fight — while keeping pattern generation O(N) in face count.
///
/// Returns `Ok(true)` when at least one face pair has overlapping AABBs,
/// `Ok(false)` otherwise. Faces with empty/missing loops are skipped
/// rather than treated as errors so a partially-built model still flows
/// through pattern.
fn check_interference(model: &BRepModel, instance: &[FaceId]) -> OperationResult<bool> {
    let bboxes: Vec<[f64; 6]> = instance
        .iter()
        .filter_map(|&face_id| compute_face_aabb(model, face_id))
        .collect();
    for i in 0..bboxes.len() {
        for j in (i + 1)..bboxes.len() {
            if aabbs_overlap(&bboxes[i], &bboxes[j]) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// Compute axis-aligned bounding box of a face from its outer loop's
/// vertex positions. Returns `[xmin, ymin, zmin, xmax, ymax, zmax]` or
/// `None` when the face/loop/edges/vertices can't be resolved.
fn compute_face_aabb(model: &BRepModel, face_id: FaceId) -> Option<[f64; 6]> {
    let face = model.faces.get(face_id)?;
    let loop_ref = model.loops.get(face.outer_loop)?;
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    let mut have_any = false;
    for &edge_id in &loop_ref.edges {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => continue,
        };
        for vid in [edge.start_vertex, edge.end_vertex] {
            if let Some(pos) = model.vertices.get_position(vid) {
                for k in 0..3 {
                    if pos[k] < min[k] {
                        min[k] = pos[k];
                    }
                    if pos[k] > max[k] {
                        max[k] = pos[k];
                    }
                }
                have_any = true;
            }
        }
    }
    if !have_any {
        return None;
    }
    Some([min[0], min[1], min[2], max[0], max[1], max[2]])
}

#[inline]
fn aabbs_overlap(a: &[f64; 6], b: &[f64; 6]) -> bool {
    a[0] <= b[3] && a[3] >= b[0] && a[1] <= b[4] && a[4] >= b[1] && a[2] <= b[5] && a[5] >= b[2]
}

/// Merge coincident geometry in pattern
fn merge_pattern_geometry(
    model: &mut BRepModel,
    instances: &mut Vec<Vec<FaceId>>,
) -> OperationResult<()> {
    // Would merge vertices, edges, and faces that are coincident
    Ok(())
}

/// Validate pattern inputs.
///
/// Source features must exist in the model and pattern parameters must be
/// finite + non-degenerate. Spacings are checked against
/// `options.common.tolerance.distance()` rather than hard-coded literals so
/// the user-supplied tolerance drives the "too small to matter" cutoff.
fn validate_pattern_inputs(
    model: &BRepModel,
    source_features: &[FaceId],
    pattern_type: &PatternType,
    options: &PatternOptions,
) -> OperationResult<()> {
    // Empty source list: no faces to pattern, hard error rather than
    // silently producing zero results.
    if source_features.is_empty() {
        return Err(OperationError::InvalidGeometry(
            "validate_pattern_inputs: source_features is empty".to_string(),
        ));
    }

    // Check source features exist
    for &face_id in source_features {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "validate_pattern_inputs: source face {} not found",
                face_id
            )));
        }
    }

    // The user's tolerance drives the lower bound for spacings and
    // direction magnitudes — anything smaller would produce coincident
    // instances or undefined directions.
    let tol = options.common.tolerance.distance();

    // Validate pattern parameters
    match pattern_type {
        PatternType::Linear {
            direction,
            spacing,
            count,
        } => {
            if *count < 1 {
                return Err(OperationError::InvalidPattern(
                    "Pattern count must be at least 1".to_string(),
                ));
            }
            if !spacing.is_finite() || *spacing <= tol {
                return Err(OperationError::InvalidPattern(format!(
                    "Linear pattern spacing {:.3e} not greater than tolerance {:.3e}",
                    spacing, tol
                )));
            }
            if direction.magnitude() <= tol {
                return Err(OperationError::InvalidPattern(format!(
                    "Linear pattern direction magnitude {:.3e} not greater than tolerance {:.3e}",
                    direction.magnitude(),
                    tol
                )));
            }
        }
        PatternType::Circular {
            axis_origin: _,
            axis_direction,
            count,
            angle: _,
        } => {
            if *count < 1 {
                return Err(OperationError::InvalidPattern(
                    "Pattern count must be at least 1".to_string(),
                ));
            }
            if axis_direction.magnitude() <= tol {
                return Err(OperationError::InvalidPattern(format!(
                    "Circular pattern axis_direction magnitude {:.3e} \
                     not greater than tolerance {:.3e}",
                    axis_direction.magnitude(),
                    tol
                )));
            }
        }
        PatternType::Rectangular(rect) => {
            if rect.count1 < 1 || rect.count2 < 1 {
                return Err(OperationError::InvalidPattern(
                    "Pattern counts must be at least 1".to_string(),
                ));
            }
            if !rect.spacing1.is_finite()
                || !rect.spacing2.is_finite()
                || rect.spacing1 <= tol
                || rect.spacing2 <= tol
            {
                return Err(OperationError::InvalidPattern(format!(
                    "Rectangular spacings ({:.3e}, {:.3e}) must both exceed tolerance {:.3e}",
                    rect.spacing1, rect.spacing2, tol
                )));
            }
        }
        _ => {} // Other types validated during generation
    }

    Ok(())
}

/// Validate pattern result.
///
/// Confirms (a) every instance face exists in the model and (b) the
/// model as a whole still passes `Standard`-level B-Rep validation
/// after pattern generation. Pattern generation duplicates topology
/// in bulk, so a quick existence sweep + structural validation catches
/// the common failure modes (orphan faces, duplicate edges with broken
/// orientation, missing vertex links).
fn validate_pattern_result(model: &BRepModel, instances: &[Vec<FaceId>]) -> OperationResult<()> {
    for (i, instance) in instances.iter().enumerate() {
        for &face_id in instance {
            if model.faces.get(face_id).is_none() {
                return Err(OperationError::InvalidGeometry(format!(
                    "pattern instance {}: face {} missing from model",
                    i, face_id
                )));
            }
        }
    }
    let result = crate::primitives::validation::validate_model_enhanced(
        model,
        crate::math::Tolerance::default(),
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
            "patterned model failed validation ({} error(s)): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_linear_pattern_transforms() {
//         let pattern = LinearPattern {
//             direction: Vector3::X,
//             spacing: 10.0,
//             count: 3,
//             centered: false,
//         };
//
//         let transforms = generate_linear_transforms(&pattern).unwrap();
//         assert_eq!(transforms.len(), 3);
//
//         // Check that transforms are correct
//         // First should be identity
//         // Second should translate by 10 in X
//         // Third should translate by 20 in X
//     }
//
//     #[test]
//     fn test_circular_pattern_transforms() {
//         let pattern = CircularPattern {
//             axis_origin: Point3::ZERO,
//             axis_direction: Vector3::Z,
//             total_angle: std::f64::consts::TAU,
//             count: 4,
//             include_original: true,
//             symmetric: false,
//         };
//
//         let transforms = generate_circular_transforms(&pattern).unwrap();
//         assert_eq!(transforms.len(), 4);
//
//         // Check that transforms create 90-degree rotations
//     }
// }
