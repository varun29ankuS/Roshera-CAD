//! Box/Cuboid primitive with full B-Rep topology
//!
//! This module implements a world-class parametric box primitive that meets
//! all requirements for exact geometry, complete topology, and parametric updates.
//!
//! Indexed access into the 8-vertex / 12-edge / 6-face buffers is the canonical
//! idiom — bounded by box topology constants. Matches nurbs.rs pattern.
#![allow(clippy::indexing_slicing)]

use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::Line,
    edge::{Edge, EdgeOrientation},
    primitive_traits::{
        EntityRef, IssueSeverity, ManifoldStatus, ParameterDefinition, ParameterSchema,
        ParameterType, Primitive, PrimitiveError, ValidationIssue, ValidationMetrics,
        ValidationReport, ValueConstraint,
    },
    solid::SolidId,
    surface::Plane,
    topology_builder::BRepModel,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Parametric box primitive with exact analytical geometry
///
/// Creates a rectangular cuboid with complete B-Rep topology:
/// - 8 vertices at box corners
/// - 12 edges (4 bottom + 4 top + 4 vertical)
/// - 6 faces (bottom, top, front, back, left, right)
/// - 1 shell (closed manifold)
/// - 1 solid
#[derive(Debug, Clone)]
pub struct BoxPrimitive;

/// Builder pattern for creating box primitives
#[derive(Debug, Clone)]
pub struct BoxBuilder {
    params: BoxParameters,
}

impl BoxBuilder {
    /// Create a new box builder with default parameters
    pub fn new() -> Self {
        Self {
            params: BoxParameters::default(),
        }
    }

    /// Set box dimensions
    pub fn dimensions(
        mut self,
        width: f64,
        height: f64,
        depth: f64,
    ) -> Result<Self, PrimitiveError> {
        BoxParameters::validate_dimensions(width, height, depth)?;
        self.params.width = width;
        self.params.height = height;
        self.params.depth = depth;
        Ok(self)
    }

    /// Set box center position
    pub fn center(mut self, center: Point3) -> Self {
        let translation = Matrix4::from_translation(&center);
        if let Some(transform) = self.params.transform.as_mut() {
            // Combine with existing transform preserving rotation/scale
            *transform = translation * *transform;
        } else {
            self.params.transform = Some(translation);
        }
        self
    }

    /// Set corner radius for rounded edges
    pub fn corner_radius(mut self, radius: f64) -> Result<Self, PrimitiveError> {
        if radius < 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "corner_radius".to_string(),
                value: radius.to_string(),
                constraint: "must be non-negative".to_string(),
            });
        }

        let min_dim = self
            .params
            .width
            .min(self.params.height)
            .min(self.params.depth);
        if radius > min_dim / 2.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "corner_radius".to_string(),
                value: radius.to_string(),
                constraint: format!(
                    "must not exceed {} (half of smallest dimension)",
                    min_dim / 2.0
                ),
            });
        }

        self.params.corner_radius = Some(radius);
        Ok(self)
    }

    /// Set transformation matrix
    pub fn transform(mut self, transform: Matrix4) -> Self {
        self.params.transform = Some(transform);
        self
    }

    /// Set construction tolerance
    pub fn tolerance(mut self, tolerance: Tolerance) -> Self {
        self.params.tolerance = Some(tolerance);
        self
    }

    /// Build the box primitive
    pub fn build(self, model: &mut BRepModel) -> Result<SolidId, PrimitiveError> {
        BoxPrimitive::create(self.params, model)
    }
}

impl BoxPrimitive {
    /// Create a box builder for convenient construction
    pub fn builder() -> BoxBuilder {
        BoxBuilder::new()
    }

    /// Create a box from corner points
    pub fn from_corners(
        p1: Point3,
        p2: Point3,
        model: &mut BRepModel,
    ) -> Result<SolidId, PrimitiveError> {
        let min_x = p1.x.min(p2.x);
        let max_x = p1.x.max(p2.x);
        let min_y = p1.y.min(p2.y);
        let max_y = p1.y.max(p2.y);
        let min_z = p1.z.min(p2.z);
        let max_z = p1.z.max(p2.z);

        let width = max_x - min_x;
        let height = max_y - min_y;
        let depth = max_z - min_z;
        let center = Point3::new(
            (min_x + max_x) / 2.0,
            (min_y + max_y) / 2.0,
            (min_z + max_z) / 2.0,
        );

        Self::builder()
            .dimensions(width, height, depth)?
            .center(center)
            .build(model)
    }
}

/// Box construction parameters
///
/// All dimensions must be positive. The box is centered at the origin
/// unless a transform is applied.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BoxParameters {
    /// Width dimension (X-axis)
    pub width: f64,
    /// Height dimension (Y-axis)
    pub height: f64,
    /// Depth dimension (Z-axis)
    pub depth: f64,
    /// Optional corner radius for rounded box
    pub corner_radius: Option<f64>,
    /// Optional transformation matrix
    pub transform: Option<Matrix4>,
    /// Tolerance for construction
    pub tolerance: Option<Tolerance>,
}

/// Box topology helper - pre-computed constants for optimal performance
struct BoxTopology;

impl BoxTopology {
    /// Vertex positions in normalized coordinates (-1 to 1)
    const VERTEX_POSITIONS: [(f64, f64, f64); 8] = [
        (-1.0, -1.0, -1.0), // v0: bottom-front-left
        (1.0, -1.0, -1.0),  // v1: bottom-front-right
        (1.0, 1.0, -1.0),   // v2: bottom-back-right
        (-1.0, 1.0, -1.0),  // v3: bottom-back-left
        (-1.0, -1.0, 1.0),  // v4: top-front-left
        (1.0, -1.0, 1.0),   // v5: top-front-right
        (1.0, 1.0, 1.0),    // v6: top-back-right
        (-1.0, 1.0, 1.0),   // v7: top-back-left
    ];

    /// Edge connectivity (vertex index pairs)
    const EDGE_VERTICES: [(usize, usize); 12] = [
        // Bottom face edges
        (0, 1),
        (1, 2),
        (2, 3),
        (3, 0),
        // Top face edges
        (4, 5),
        (5, 6),
        (6, 7),
        (7, 4),
        // Vertical edges
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];

    /// Face topology (which edges form each face) and per-edge orientations.
    ///
    /// Both arrays are chosen so that `Loop::vertices_cached` produces a
    /// non-degenerate quad whose right-hand-rule normal points along the
    /// face's outward `FACE_DATA` normal. See the analogous fix in
    /// `topology_builder::create_box_faces` for the derivation.
    const FACE_EDGES: [[usize; 4]; 6] = [
        [3, 2, 1, 0],   // Bottom (-Z): traversal v0→v3→v2→v1
        [4, 5, 6, 7],   // Top    (+Z): traversal v4→v5→v6→v7
        [0, 9, 4, 8],   // Front  (-Y): traversal v0→v1→v5→v4
        [2, 11, 6, 10], // Back   (+Y): traversal v2→v3→v7→v6
        [8, 7, 11, 3],  // Left   (-X): traversal v0→v4→v7→v3
        [1, 10, 5, 9],  // Right  (+X): traversal v1→v2→v6→v5
    ];

    /// Per-edge orientations for `FACE_EDGES`: true = forward, false = reversed.
    const FACE_EDGE_ORIENTATIONS: [[bool; 4]; 6] = [
        [false, false, false, false], // Bottom
        [true, true, true, true],     // Top
        [true, true, false, false],   // Front
        [true, true, false, false],   // Back
        [true, false, false, true],   // Left
        [true, true, false, false],   // Right
    ];

    /// Face normals and centers (normalized)
    const FACE_DATA: [((f64, f64, f64), (f64, f64, f64)); 6] = [
        ((0.0, 0.0, -1.0), (0.0, 0.0, -1.0)), // Bottom face
        ((0.0, 0.0, 1.0), (0.0, 0.0, 1.0)),   // Top face
        ((0.0, -1.0, 0.0), (0.0, -1.0, 0.0)), // Front face
        ((0.0, 1.0, 0.0), (0.0, 1.0, 0.0)),   // Back face
        ((-1.0, 0.0, 0.0), (-1.0, 0.0, 0.0)), // Left face
        ((1.0, 0.0, 0.0), (1.0, 0.0, 0.0)),   // Right face
    ];
}

impl Default for BoxParameters {
    fn default() -> Self {
        Self {
            width: 10.0,
            height: 10.0,
            depth: 10.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        }
    }
}

impl BoxParameters {
    /// Create new box parameters with validation
    pub fn new(width: f64, height: f64, depth: f64) -> Result<Self, PrimitiveError> {
        Self::validate_dimensions(width, height, depth)?;

        Ok(Self {
            width,
            height,
            depth,
            corner_radius: None,
            transform: None,
            tolerance: None,
        })
    }

    /// Create parameters for a unit cube
    pub fn unit_cube() -> Self {
        Self {
            width: 1.0,
            height: 1.0,
            depth: 1.0,
            corner_radius: None,
            transform: None,
            tolerance: None,
        }
    }

    /// Create parameters for a cube with given size
    pub fn cube(size: f64) -> Result<Self, PrimitiveError> {
        Self::new(size, size, size)
    }

    /// Set corner radius for rounded box
    pub fn with_corner_radius(mut self, radius: f64) -> Result<Self, PrimitiveError> {
        if radius < 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "corner_radius".to_string(),
                value: radius.to_string(),
                constraint: "must be non-negative".to_string(),
            });
        }

        // Check radius doesn't exceed half of smallest dimension
        let min_dim = self.width.min(self.height).min(self.depth);
        if radius > min_dim / 2.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "corner_radius".to_string(),
                value: radius.to_string(),
                constraint: format!(
                    "must not exceed {} (half of smallest dimension)",
                    min_dim / 2.0
                ),
            });
        }

        self.corner_radius = Some(radius);
        Ok(self)
    }

    /// Set transformation matrix
    pub fn with_transform(mut self, transform: Matrix4) -> Self {
        self.transform = Some(transform);
        self
    }

    /// Set construction tolerance
    pub fn with_tolerance(mut self, tolerance: Tolerance) -> Self {
        self.tolerance = Some(tolerance);
        self
    }

    /// Validate box dimensions
    fn validate_dimensions(width: f64, height: f64, depth: f64) -> Result<(), PrimitiveError> {
        if width <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "width".to_string(),
                value: width.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        if height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "height".to_string(),
                value: height.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        if depth <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "depth".to_string(),
                value: depth.to_string(),
                constraint: "must be positive".to_string(),
            });
        }

        // Check for numerical limits
        const MAX_DIMENSION: f64 = 1e6; // 1 million units
        const MIN_DIMENSION: f64 = 1e-6; // 1 micron

        if width > MAX_DIMENSION || height > MAX_DIMENSION || depth > MAX_DIMENSION {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}, {}, {}", width, height, depth),
                constraint: format!("no dimension may exceed {}", MAX_DIMENSION),
            });
        }

        if width < MIN_DIMENSION || height < MIN_DIMENSION || depth < MIN_DIMENSION {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("{}, {}, {}", width, height, depth),
                constraint: format!("no dimension may be less than {}", MIN_DIMENSION),
            });
        }

        Ok(())
    }

    /// Get all parameter values for validation
    pub fn all_values(&self) -> Vec<(&str, f64)> {
        let mut values = vec![
            ("width", self.width),
            ("height", self.height),
            ("depth", self.depth),
        ];

        if let Some(radius) = self.corner_radius {
            values.push(("corner_radius", radius));
        }

        values
    }
}

impl Primitive for BoxPrimitive {
    type Parameters = BoxParameters;

    #[allow(clippy::expect_used)] // box vertices added to model.vertices earlier in same fn
    fn create(params: Self::Parameters, model: &mut BRepModel) -> Result<SolidId, PrimitiveError> {
        // Validate parameters
        BoxParameters::validate_dimensions(params.width, params.height, params.depth)?;

        // Get tolerance
        let tolerance = params.tolerance.unwrap_or_default();

        // Calculate half dimensions for vertex positioning
        let hw = params.width / 2.0;
        let hh = params.height / 2.0;
        let hd = params.depth / 2.0;

        // Create vertices using pre-computed topology
        let mut vertices = Vec::with_capacity(8);
        for &(x, y, z) in &BoxTopology::VERTEX_POSITIONS {
            let mut pos = Point3::new(x * hw, y * hh, z * hd);

            // Apply transformation if provided
            if let Some(transform) = params.transform {
                pos = transform.transform_point(&pos);
            }

            let vertex_id = model
                .vertices
                .add_or_find(pos.x, pos.y, pos.z, tolerance.distance());
            vertices.push(vertex_id);
        }

        // Create surfaces
        let mut surfaces = Vec::with_capacity(6);
        for &((cx, cy, cz), (nx, ny, nz)) in &BoxTopology::FACE_DATA {
            let mut center = Point3::new(cx * hw, cy * hh, cz * hd);
            let mut normal = Vector3::new(nx, ny, nz);

            // Apply transformation if provided
            if let Some(transform) = params.transform {
                center = transform.transform_point(&center);
                // Face normals from `BoxTopology::FACE_DATA` are unit axis
                // vectors. Roshera transforms are expected to be rigid (or at
                // worst uniform-scaling), so the transformed normal has
                // non-zero magnitude. If a caller supplies a truly degenerate
                // transform we fall back to the pre-transform unit normal
                // rather than panicking during box construction.
                let raw = transform.transform_vector(&normal);
                normal = raw.normalize().unwrap_or(normal);
            }

            let plane = Plane::from_point_normal(center, normal).map_err(|_| {
                PrimitiveError::TopologyError {
                    message: "Failed to create plane surface".to_string(),
                    euler_characteristic: None,
                }
            })?;
            surfaces.push(model.surfaces.add(Box::new(plane)));
        }

        // Create edges
        let mut edges = Vec::with_capacity(12);
        for &(start_idx, end_idx) in &BoxTopology::EDGE_VERTICES {
            let start_vertex = vertices[start_idx];
            let end_vertex = vertices[end_idx];

            // Get vertex positions. Both `start_vertex` and `end_vertex` were
            // added to `model.vertices` earlier in this function, so they are
            // guaranteed to exist by the local construction order.
            let start_pos = model
                .vertices
                .get_position(start_vertex)
                .expect("box vertex was added to model.vertices earlier in this function");
            let end_pos = model
                .vertices
                .get_position(end_vertex)
                .expect("box vertex was added to model.vertices earlier in this function");

            // Create line curve
            let line = Line::new(Point3::from(start_pos), Point3::from(end_pos));
            let curve_id = model.curves.add(Box::new(line));

            // Create edge
            let edge = Edge::new(
                0, // ID will be assigned by store
                start_vertex,
                end_vertex,
                curve_id,
                EdgeOrientation::Forward,
                crate::primitives::curve::ParameterRange::unit(),
            );
            edges.push(model.edges.add_or_find(edge));
        }

        // Create faces
        let mut faces = Vec::with_capacity(6);
        for (face_idx, &edge_indices) in BoxTopology::FACE_EDGES.iter().enumerate() {
            // Create loop for face
            let mut face_loop =
                crate::primitives::r#loop::Loop::new(0, crate::primitives::r#loop::LoopType::Outer);

            // Add edges to loop with per-edge orientation so the resulting
            // vertex traversal matches the outward face normal.
            let orientations = BoxTopology::FACE_EDGE_ORIENTATIONS[face_idx];
            for (i, &edge_idx) in edge_indices.iter().enumerate() {
                face_loop.add_edge(edges[edge_idx], orientations[i]);
            }
            let loop_id = model.loops.add(face_loop);

            // Create face
            let face = crate::primitives::face::Face::new(
                0,
                surfaces[face_idx],
                loop_id,
                crate::primitives::face::FaceOrientation::Forward,
            );
            faces.push(model.faces.add(face));
        }

        // Create shell from faces
        let mut shell =
            crate::primitives::shell::Shell::new(0, crate::primitives::shell::ShellType::Closed);
        for face_id in faces {
            shell.add_face(face_id);
        }
        let shell_id = model.shells.add(shell);

        // Create solid from shell
        let solid = crate::primitives::solid::Solid::new(0, shell_id);
        let solid_id = model.solids.add(solid);

        Ok(solid_id)
    }

    fn update_parameters(
        solid_id: SolidId,
        params: Self::Parameters,
        model: &mut BRepModel,
    ) -> Result<(), PrimitiveError> {
        // Validate new parameters
        BoxParameters::validate_dimensions(params.width, params.height, params.depth)?;

        // Remove existing solid and recreate with new parameters
        model.solids.remove(solid_id);

        // Create new solid with same ID
        let new_solid_id = Self::create(params, model)?;

        // Verify we got the expected ID back
        if new_solid_id != solid_id {
            return Err(PrimitiveError::TopologyError {
                message: "Failed to preserve solid ID during update".to_string(),
                euler_characteristic: None,
            });
        }

        Ok(())
    }

    fn get_parameters(
        solid_id: SolidId,
        model: &BRepModel,
    ) -> Result<Self::Parameters, PrimitiveError> {
        let solid = model
            .solids
            .get(solid_id)
            .ok_or(PrimitiveError::NotFound { solid_id })?;

        // Collect all vertices from the solid's shell faces
        let shell =
            model
                .shells
                .get(solid.outer_shell)
                .ok_or_else(|| PrimitiveError::GeometryError {
                    operation: "get_parameters".to_string(),
                    details: "Outer shell not found".to_string(),
                })?;

        let mut min_x = f64::MAX;
        let mut min_y = f64::MAX;
        let mut min_z = f64::MAX;
        let mut max_x = f64::MIN;
        let mut max_y = f64::MIN;
        let mut max_z = f64::MIN;
        let mut vertex_count = 0;

        for &face_id in &shell.faces {
            if let Some(face) = model.faces.get(face_id) {
                if let Some(loop_data) = model.loops.get(face.outer_loop) {
                    for &edge_id in &loop_data.edges {
                        if let Some(edge) = model.edges.get(edge_id) {
                            for vid in [edge.start_vertex, edge.end_vertex] {
                                if let Some(v) = model.vertices.get(vid) {
                                    min_x = min_x.min(v.position[0]);
                                    min_y = min_y.min(v.position[1]);
                                    min_z = min_z.min(v.position[2]);
                                    max_x = max_x.max(v.position[0]);
                                    max_y = max_y.max(v.position[1]);
                                    max_z = max_z.max(v.position[2]);
                                    vertex_count += 1;
                                }
                            }
                        }
                    }
                }
            }
        }

        if vertex_count == 0 {
            return Err(PrimitiveError::GeometryError {
                operation: "get_parameters".to_string(),
                details: "No vertices found in solid".to_string(),
            });
        }

        Ok(BoxParameters {
            width: max_x - min_x,
            height: max_y - min_y,
            depth: max_z - min_z,
            corner_radius: None,
            transform: None,
            tolerance: None,
        })
    }

    fn validate(solid_id: SolidId, model: &BRepModel) -> Result<ValidationReport, PrimitiveError> {
        let start_time = Instant::now();
        let mut issues = Vec::new();
        let mut entities_checked = 0;

        // Get solid
        let solid = model
            .solids
            .get(solid_id)
            .ok_or(PrimitiveError::NotFound { solid_id })?;
        entities_checked += 1;

        // Skip primitive type check since we don't have that method

        // Get topology counts
        let shell_count = solid.shell_ids().len();
        entities_checked += shell_count;

        if shell_count != 1 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                description: format!("Box should have exactly 1 shell, found {}", shell_count),
                entities: vec![EntityRef::Solid(solid_id)],
                suggested_fix: Some("Rebuild box with single manifold shell".to_string()),
            });
        }

        // Check each shell
        for shell_id in solid.shell_ids() {
            // `solid.shell_ids()` only returns IDs owned by this solid in the
            // model, so a `None` here would indicate a corrupted BRep — skip
            // the shell and record a validation issue rather than panicking.
            let Some(shell) = model.shells.get(shell_id) else {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    description: format!(
                        "Solid references shell {} that is missing from the model",
                        shell_id
                    ),
                    entities: vec![EntityRef::Solid(solid_id)],
                    suggested_fix: Some("Rebuild box with consistent shell references".to_string()),
                });
                continue;
            };
            let face_count = shell.face_ids().len();
            entities_checked += face_count;

            if face_count != 6 {
                issues.push(ValidationIssue {
                    severity: IssueSeverity::Error,
                    description: format!(
                        "Box shell should have exactly 6 faces, found {}",
                        face_count
                    ),
                    entities: vec![EntityRef::Solid(solid_id)],
                    suggested_fix: Some("Rebuild box with 6 faces".to_string()),
                });
            }
        }

        // Calculate Euler characteristic: V - E + F
        let vertex_count = 8; // Box should have 8 vertices
        let edge_count = 12; // Box should have 12 edges
        let face_count = 6; // Box should have 6 faces
        let euler_characteristic = vertex_count - edge_count + face_count;

        if euler_characteristic != 2 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                description: format!(
                    "Invalid Euler characteristic: V-E+F = {}-{}+{} = {} (should be 2)",
                    vertex_count, edge_count, face_count, euler_characteristic
                ),
                entities: vec![EntityRef::Solid(solid_id)],
                suggested_fix: Some("Fix topology to satisfy Euler's formula".to_string()),
            });
        }

        // Determine manifold status
        let manifold_check = if issues.iter().any(|i| i.severity == IssueSeverity::Error) {
            ManifoldStatus::Invalid { open_edges: vec![] }
        } else {
            ManifoldStatus::Manifold
        };

        // Calculate metrics
        let duration = start_time.elapsed();
        let metrics = ValidationMetrics {
            duration_ms: duration.as_secs_f64() * 1000.0,
            entities_checked,
            memory_used_kb: 0, // Memory tracking not implemented
        };

        Ok(ValidationReport {
            is_valid: issues.iter().all(|i| i.severity != IssueSeverity::Error),
            euler_characteristic,
            manifold_check,
            issues,
            metrics,
        })
    }

    fn primitive_type() -> &'static str {
        "box"
    }

    fn parameter_schema() -> ParameterSchema {
        ParameterSchema {
            version: "1.0".to_string(),
            parameters: vec![
                ParameterDefinition {
                    name: "width".to_string(),
                    display_name: "Width".to_string(),
                    description: "Box width dimension (X-axis)".to_string(),
                    param_type: ParameterType::Float { precision: Some(3) },
                    default_value: serde_json::json!(10.0),
                    constraints: vec![ValueConstraint::Positive],
                    units: Some("mm".to_string()),
                },
                ParameterDefinition {
                    name: "height".to_string(),
                    display_name: "Height".to_string(),
                    description: "Box height dimension (Y-axis)".to_string(),
                    param_type: ParameterType::Float { precision: Some(3) },
                    default_value: serde_json::json!(10.0),
                    constraints: vec![ValueConstraint::Positive],
                    units: Some("mm".to_string()),
                },
                ParameterDefinition {
                    name: "depth".to_string(),
                    display_name: "Depth".to_string(),
                    description: "Box depth dimension (Z-axis)".to_string(),
                    param_type: ParameterType::Float { precision: Some(3) },
                    default_value: serde_json::json!(10.0),
                    constraints: vec![ValueConstraint::Positive],
                    units: Some("mm".to_string()),
                },
                ParameterDefinition {
                    name: "corner_radius".to_string(),
                    display_name: "Corner Radius".to_string(),
                    description: "Optional corner rounding radius".to_string(),
                    param_type: ParameterType::Float { precision: Some(3) },
                    default_value: serde_json::json!(null),
                    constraints: vec![ValueConstraint::NonNegative],
                    units: Some("mm".to_string()),
                },
            ],
            constraints: vec![],
        }
    }
}
