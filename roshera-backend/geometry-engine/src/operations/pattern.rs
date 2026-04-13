//! Pattern Operations for B-Rep Models
//!
//! Creates arrays and patterns of features including linear, circular,
//! rectangular, and custom patterns.

use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Curve,
    edge::{Edge, EdgeId},
    face::{Face, FaceId},
    r#loop::Loop,
    shell::Shell,
    solid::{Solid, SolidId},
    surface::Surface,
    topology_builder::BRepModel,
    vertex::{Vertex, VertexId},
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

/// Generate transforms for curve pattern
fn generate_curve_transforms(pattern: &CurvePattern) -> OperationResult<Vec<Matrix4>> {
    // Would generate transforms along curve
    // For now, return placeholder
    Err(OperationError::NotImplemented(
        "Curve pattern not yet implemented".to_string(),
    ))
}

/// Create a single pattern instance
fn create_pattern_instance(
    model: &mut BRepModel,
    source_features: &[FaceId],
    transform: &Matrix4,
    instance_index: usize,
    options: &PatternOptions,
) -> OperationResult<Vec<FaceId>> {
    let mut instance_faces = Vec::new();

    for &face_id in source_features {
        let transformed_face = transform_face(model, face_id, transform)?;
        instance_faces.push(transformed_face);
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

/// Check for interference between instances
fn check_interference(model: &BRepModel, instance: &[FaceId]) -> OperationResult<bool> {
    // Would check for geometric interference
    // For now, always return false (no interference)
    Ok(false)
}

/// Merge coincident geometry in pattern
fn merge_pattern_geometry(
    model: &mut BRepModel,
    instances: &mut Vec<Vec<FaceId>>,
) -> OperationResult<()> {
    // Would merge vertices, edges, and faces that are coincident
    Ok(())
}

/// Validate pattern inputs
fn validate_pattern_inputs(
    model: &BRepModel,
    source_features: &[FaceId],
    pattern_type: &PatternType,
    options: &PatternOptions,
) -> OperationResult<()> {
    // Check source features exist
    for &face_id in source_features {
        if model.faces.get(face_id).is_none() {
            return Err(OperationError::InvalidGeometry(
                "Source face not found".to_string(),
            ));
        }
    }

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
            if *spacing <= 0.0 {
                return Err(OperationError::InvalidPattern(
                    "Pattern spacing must be positive".to_string(),
                ));
            }
            if direction.magnitude() < 1e-10 {
                return Err(OperationError::InvalidPattern(
                    "Invalid pattern direction".to_string(),
                ));
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
            if axis_direction.magnitude() < 1e-10 {
                return Err(OperationError::InvalidPattern(
                    "Invalid axis direction".to_string(),
                ));
            }
        }
        PatternType::Rectangular(rect) => {
            if rect.count1 < 1 || rect.count2 < 1 {
                return Err(OperationError::InvalidPattern(
                    "Pattern counts must be at least 1".to_string(),
                ));
            }
            if rect.spacing1 <= 0.0 || rect.spacing2 <= 0.0 {
                return Err(OperationError::InvalidPattern(
                    "Pattern spacings must be positive".to_string(),
                ));
            }
        }
        _ => {} // Other types validated during generation
    }

    Ok(())
}

/// Validate pattern result
fn validate_pattern_result(model: &BRepModel, instances: &[Vec<FaceId>]) -> OperationResult<()> {
    // Would validate that pattern created valid geometry
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
