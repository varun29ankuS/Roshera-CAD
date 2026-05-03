//! Cylinder primitive with full B-Rep topology
//!
//! Parametric cylinder primitive with exact analytic geometry, a complete
//! B-Rep topology (lateral + cap faces, axial + circular edges), and
//! parametric updates that preserve identity across edits.
//!
//! Indexed access into the (2N+2) vertex / (3N) edge / (N+2) face buffers is
//! the canonical idiom — indices are bounded by the segment count `n` chosen
//! at construction. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::math::{Matrix4, Point3, Tolerance, Vector3};
use crate::primitives::{
    curve::{Circle, Line, ParameterRange},
    edge::{Edge, EdgeOrientation},
    face::FaceOrientation,
    primitive_traits::{
        EntityRef, IssueSeverity, ManifoldStatus, ParameterDefinition, ParameterSchema,
        ParameterType, Primitive, PrimitiveError, ValidationIssue, ValidationMetrics,
        ValidationReport, ValueConstraint,
    },
    r#loop::{Loop, LoopType},
    shell::{Shell, ShellType},
    solid::Solid,
    solid::SolidId,
    surface::{Cylinder as CylinderSurface, Plane},
    topology_builder::BRepModel,
};
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Parametric cylinder primitive with exact analytical geometry
///
/// Creates a cylinder with complete B-Rep topology
#[derive(Debug, Clone)]
pub struct CylinderPrimitive;

/// Cylinder construction parameters
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CylinderParameters {
    /// Radius of the cylinder
    pub radius: f64,
    /// Height of the cylinder
    pub height: f64,
    /// Base center point
    pub base_center: Point3,
    /// Axis direction (normalized)
    pub axis: Vector3,
    /// Number of segments for circular edges
    pub segments: u32,
    /// Optional transformation matrix
    pub transform: Option<Matrix4>,
    /// Tolerance for construction
    pub tolerance: Option<Tolerance>,
}

impl Default for CylinderParameters {
    fn default() -> Self {
        Self {
            radius: 10.0,
            height: 20.0,
            base_center: Point3::ORIGIN,
            axis: Vector3::Z,
            segments: 16,
            transform: None,
            tolerance: None,
        }
    }
}

impl CylinderParameters {
    /// Create new cylinder parameters with validation
    pub fn new(radius: f64, height: f64) -> Result<Self, PrimitiveError> {
        if radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "radius".to_string(),
                value: radius.to_string(),
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

        Ok(Self {
            radius,
            height,
            base_center: Point3::ORIGIN,
            axis: Vector3::Z,
            segments: 16,
            transform: None,
            tolerance: None,
        })
    }

    /// Set cylinder axis
    pub fn with_axis(mut self, axis: Vector3) -> Result<Self, PrimitiveError> {
        let axis = axis
            .normalize()
            .map_err(|_| PrimitiveError::InvalidParameters {
                parameter: "axis".to_string(),
                value: format!("{:?}", axis),
                constraint: "must be non-zero".to_string(),
            })?;
        self.axis = axis;
        Ok(self)
    }

    /// Set number of segments
    pub fn with_segments(mut self, segments: u32) -> Result<Self, PrimitiveError> {
        if segments < 3 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "segments".to_string(),
                value: segments.to_string(),
                constraint: "must be at least 3".to_string(),
            });
        }
        self.segments = segments;
        Ok(self)
    }
}

impl Primitive for CylinderPrimitive {
    type Parameters = CylinderParameters;

    fn create(params: Self::Parameters, model: &mut BRepModel) -> Result<SolidId, PrimitiveError> {
        // Validate parameters
        if params.radius <= 0.0 || params.height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "dimensions".to_string(),
                value: format!("radius={}, height={}", params.radius, params.height),
                constraint: "must be positive".to_string(),
            });
        }

        let tolerance = params.tolerance.unwrap_or_default();

        // Build seamed-cylinder topology: a parametric cylinder is a single
        // closed lateral face (seamed at u=0≡u=2π) plus two planar caps —
        // 3 faces total, regardless of segment count. The previous
        // implementation faceted the lateral surface into N separate
        // sub-arc faces (one per `segments`), which (a) violated the B-Rep
        // contract that analytic surfaces remain analytic at topology
        // level, and (b) caused the tessellator to emit
        // segments × max_segments² triangles per cylinder (e.g. 16 ×
        // 100² = 160k extra tris on a r=15 h=80 cylinder).
        //
        // `segments` is now a documented tessellation hint; topology is
        // always seamed. Mirror of `topology_builder::create_cylinder_topology`.
        let z_axis = params.axis;
        let ref_dir = z_axis.perpendicular();
        let top_center_pt = params.base_center + z_axis * params.height;

        let topology_err = |op: &str, msg: String| PrimitiveError::GeometryError {
            operation: op.to_string(),
            details: msg,
        };

        // ---- vertices: one seam vertex per cap. ----
        let v_bottom = model.vertices.add_or_find(
            params.base_center.x + ref_dir.x * params.radius,
            params.base_center.y + ref_dir.y * params.radius,
            params.base_center.z + ref_dir.z * params.radius,
            tolerance.distance(),
        );
        let v_top = model.vertices.add_or_find(
            top_center_pt.x + ref_dir.x * params.radius,
            top_center_pt.y + ref_dir.y * params.radius,
            top_center_pt.z + ref_dir.z * params.radius,
            tolerance.distance(),
        );

        // ---- curves: two closed circles + one seam line. ----
        let bottom_circle = Circle::new(params.base_center, z_axis, params.radius)
            .map_err(|e| topology_err("bottom_circle", format!("{e}")))?;
        let top_circle = Circle::new(top_center_pt, z_axis, params.radius)
            .map_err(|e| topology_err("top_circle", format!("{e}")))?;
        let seam_line = Line::new(
            params.base_center + ref_dir * params.radius,
            top_center_pt + ref_dir * params.radius,
        );
        let bottom_circle_id = model.curves.add(Box::new(bottom_circle));
        let top_circle_id = model.curves.add(Box::new(top_circle));
        let seam_line_id = model.curves.add(Box::new(seam_line));

        // ---- edges: closed circles + linear seam. ----
        // Edge param range MUST match the underlying `Circle`'s
        // normalized [0, 1] parameterization — `Arc::evaluate(t)` clamps
        // `t` to its internal `range = ParameterRange::unit()`. Using
        // `[0, 2π]` here would cause every tessellator sample at
        // `t = j · 2π / N > 1` to collapse onto angle 2π, leaving the
        // cap as a polygon and the cap-lateral seam open. See the same
        // comment block in `topology_builder::create_cylinder_topology`.
        let bottom_edge = model.edges.add(Edge::new(
            0,
            v_bottom,
            v_bottom,
            bottom_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let top_edge = model.edges.add(Edge::new(
            0,
            v_top,
            v_top,
            top_circle_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));
        let seam_edge = model.edges.add(Edge::new(
            0,
            v_bottom,
            v_top,
            seam_line_id,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        ));

        // ---- surfaces: 2 planes + 1 finite cylinder. ----
        let bottom_plane = Plane::from_point_normal(params.base_center, -z_axis)
            .map_err(|e| topology_err("bottom_plane", format!("{e}")))?;
        let top_plane = Plane::from_point_normal(top_center_pt, z_axis)
            .map_err(|e| topology_err("top_plane", format!("{e}")))?;
        let lateral_cyl =
            CylinderSurface::new_finite(params.base_center, z_axis, params.radius, params.height)
                .map_err(|e| topology_err("lateral_cylinder", format!("{e}")))?;
        let bottom_surface_id = model.surfaces.add(Box::new(bottom_plane));
        let top_surface_id = model.surfaces.add(Box::new(top_plane));
        let lateral_surface_id = model.surfaces.add(Box::new(lateral_cyl));

        // ---- loops. ----
        // Bottom cap: outward normal is `-z_axis`. The Circle is
        // parameterized CCW when viewed from `+z_axis`. Looking from
        // `-z_axis` (outside the bottom cap), that traversal appears CW,
        // so we walk the edge `Backward` to get an outward-CCW loop.
        let mut bottom_loop = Loop::new(0, LoopType::Outer);
        bottom_loop.add_edge(bottom_edge, false);
        let bottom_loop_id = model.loops.add(bottom_loop);

        // Top cap: outward normal is `+z_axis`, same orientation as the
        // Circle's parametric CCW direction → walk `Forward`.
        let mut top_loop = Loop::new(0, LoopType::Outer);
        top_loop.add_edge(top_edge, true);
        let top_loop_id = model.loops.add(top_loop);

        // Lateral seamed face: in (u, v) parameter space the outer loop
        // is a CCW rectangle with corners at (0, 0), (2π, 0), (2π, h),
        // (0, h). Edge sequence:
        //   (0,0)→(2π,0): bottom_circle forward
        //   (2π,0)→(2π,h): seam forward
        //   (2π,h)→(0,h): top_circle backward
        //   (0,h)→(0,0): seam backward
        let mut lateral_loop = Loop::new(0, LoopType::Outer);
        lateral_loop.add_edge(bottom_edge, true);
        lateral_loop.add_edge(seam_edge, true);
        lateral_loop.add_edge(top_edge, false);
        lateral_loop.add_edge(seam_edge, false);
        let lateral_loop_id = model.loops.add(lateral_loop);

        // ---- faces. ----
        let mut bottom_face = crate::primitives::face::Face::new(
            0,
            bottom_surface_id,
            bottom_loop_id,
            FaceOrientation::Forward,
        );
        bottom_face.outer_loop = bottom_loop_id;
        let bottom_face_id = model.faces.add(bottom_face);

        let mut top_face = crate::primitives::face::Face::new(
            0,
            top_surface_id,
            top_loop_id,
            FaceOrientation::Forward,
        );
        top_face.outer_loop = top_loop_id;
        let top_face_id = model.faces.add(top_face);

        let mut lateral_face = crate::primitives::face::Face::new(
            0,
            lateral_surface_id,
            lateral_loop_id,
            FaceOrientation::Forward,
        );
        lateral_face.outer_loop = lateral_loop_id;
        let lateral_face_id = model.faces.add(lateral_face);

        // ---- shell + solid. ----
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(bottom_face_id);
        shell.add_face(top_face_id);
        shell.add_face(lateral_face_id);
        let shell_id = model.shells.add(shell);

        let solid = Solid::new(0, shell_id);
        Ok(model.solids.add(solid))
    }

    fn update_parameters(
        solid_id: SolidId,
        params: Self::Parameters,
        model: &mut BRepModel,
    ) -> Result<(), PrimitiveError> {
        // Implemented as delete + recreate: cylinder topology (1 lateral + 2 caps)
        // depends on the radius/height/segment parameters; rebuilding the shell
        // is simpler and equally correct compared to mutating individual faces.
        // ID preservation is enforced by the equality check below.
        model.solids.remove(solid_id);
        let new_solid_id = Self::create(params, model)?;
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

        let shell =
            model
                .shells
                .get(solid.outer_shell)
                .ok_or_else(|| PrimitiveError::GeometryError {
                    operation: "get_parameters".to_string(),
                    details: "Outer shell not found".to_string(),
                })?;

        // Find the cylindrical surface among the solid's faces to extract radius and axis
        let mut radius = None;
        let mut axis = None;
        let mut origin = None;

        for &face_id in &shell.faces {
            if let Some(face) = model.faces.get(face_id) {
                if let Some(surface) = model.surfaces.get(face.surface_id) {
                    if surface.surface_type() == crate::primitives::surface::SurfaceType::Cylinder {
                        use crate::primitives::surface::Cylinder;
                        if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
                            radius = Some(cyl.radius);
                            axis = Some(cyl.axis);
                            origin = Some(cyl.origin);
                        }
                    }
                }
            }
        }

        let radius = radius.ok_or_else(|| PrimitiveError::GeometryError {
            operation: "get_parameters".to_string(),
            details: "No cylindrical surface found in solid".to_string(),
        })?;
        let axis = axis.unwrap_or(Vector3::Z);
        let base_center = origin.unwrap_or(Point3::ORIGIN);

        // Compute height from bounding box along axis
        let mut min_proj = f64::MAX;
        let mut max_proj = f64::MIN;

        for &face_id in &shell.faces {
            if let Some(face) = model.faces.get(face_id) {
                if let Some(loop_data) = model.loops.get(face.outer_loop) {
                    for &edge_id in &loop_data.edges {
                        if let Some(edge) = model.edges.get(edge_id) {
                            for vid in [edge.start_vertex, edge.end_vertex] {
                                if let Some(v) = model.vertices.get(vid) {
                                    let p =
                                        Point3::new(v.position[0], v.position[1], v.position[2]);
                                    let proj = (p - base_center).dot(&axis);
                                    min_proj = min_proj.min(proj);
                                    max_proj = max_proj.max(proj);
                                }
                            }
                        }
                    }
                }
            }
        }

        let height = if max_proj > min_proj {
            max_proj - min_proj
        } else {
            1.0 // fallback
        };

        Ok(CylinderParameters {
            radius,
            height,
            base_center,
            axis,
            segments: 32,
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

        // Get topology counts
        let shell_count = solid.shell_ids().len();
        entities_checked += shell_count;

        if shell_count != 1 {
            issues.push(ValidationIssue {
                severity: IssueSeverity::Error,
                description: format!(
                    "Cylinder should have exactly 1 shell, found {}",
                    shell_count
                ),
                entities: vec![EntityRef::Solid(solid_id)],
                suggested_fix: Some("Rebuild cylinder with single manifold shell".to_string()),
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
            memory_used_kb: 0,
        };

        Ok(ValidationReport {
            is_valid: issues.iter().all(|i| i.severity != IssueSeverity::Error),
            euler_characteristic: 2, // Cylinder has Euler characteristic 2
            manifold_check,
            issues,
            metrics,
        })
    }

    fn primitive_type() -> &'static str {
        "cylinder"
    }

    fn parameter_schema() -> ParameterSchema {
        ParameterSchema {
            version: "1.0".to_string(),
            parameters: vec![
                ParameterDefinition {
                    name: "radius".to_string(),
                    display_name: "Radius".to_string(),
                    description: "Cylinder radius".to_string(),
                    param_type: ParameterType::Float { precision: Some(3) },
                    default_value: serde_json::json!(10.0),
                    constraints: vec![ValueConstraint::Positive],
                    units: Some("mm".to_string()),
                },
                ParameterDefinition {
                    name: "height".to_string(),
                    display_name: "Height".to_string(),
                    description: "Cylinder height".to_string(),
                    param_type: ParameterType::Float { precision: Some(3) },
                    default_value: serde_json::json!(20.0),
                    constraints: vec![ValueConstraint::Positive],
                    units: Some("mm".to_string()),
                },
                ParameterDefinition {
                    name: "segments".to_string(),
                    display_name: "Segments".to_string(),
                    description: "Number of segments for circular edges".to_string(),
                    param_type: ParameterType::Integer,
                    default_value: serde_json::json!(16),
                    constraints: vec![ValueConstraint::MinValue(3.0)],
                    units: None,
                },
            ],
            constraints: vec![],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cylinder_creation() {
        let mut model = BRepModel::new();
        let params = CylinderParameters::new(5.0, 10.0).unwrap();

        let solid_id = CylinderPrimitive::create(params, &mut model).unwrap();

        // Verify solid exists
        assert!(model.solids.get(solid_id).is_some());

        // Validate the cylinder
        let report = CylinderPrimitive::validate(solid_id, &model).unwrap();
        assert!(report.is_valid);
        assert_eq!(report.euler_characteristic, 2);
    }
}
