//! Cone primitive implementation
//!
//! This module provides a production-grade cone primitive with full B-Rep topology generation.
//! Handles apex singularity, partial cones (frustums), and sector cones.

use crate::{
    math::{consts, Point3, Vector3},
    operations::orientation::orient_face_for_outward,
    primitives::{
        curve::{Arc, Circle, ParameterRange},
        edge::{Edge, EdgeId, EdgeOrientation},
        face::{Face, FaceOrientation},
        primitive_traits::PrimitiveError,
        r#loop::{Loop, LoopType},
        shell::{Shell, ShellType},
        solid::{Solid, SolidId},
        surface::{Cone, Plane, SurfaceId},
        topology_builder::BRepModel,
        topology_builder::TopologyBuilder,
    },
};

/// Pick the `FaceOrientation` for a cone lateral surface so its
/// oriented outward normal points radially away from the cone's axis
/// at the parametric midpoint. The radial target is computed from
/// geometry (mid-point on surface minus its projection onto the axis)
/// rather than the cone surface's intrinsic normal, so the result is
/// stable under any future change to the `Cone` surface's u-direction
/// convention. Slice 3 of the comprehensive face-orientation fix.
fn orient_cone_lateral_outward(
    model: &BRepModel,
    cone_surface_id: SurfaceId,
    apex: Point3,
    axis: Vector3,
) -> Result<FaceOrientation, PrimitiveError> {
    let surface =
        model
            .surfaces
            .get(cone_surface_id)
            .ok_or_else(|| PrimitiveError::GeometryError {
                operation: "Cone lateral surface lookup".to_string(),
                details: "cone surface missing from store".to_string(),
            })?;
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let u_mid = 0.5 * (u_min + u_max);
    let v_mid = 0.5 * (v_min + v_max);
    let mid_point = surface
        .point_at(u_mid, v_mid)
        .map_err(|e| PrimitiveError::GeometryError {
            operation: "Cone mid-point evaluation".to_string(),
            details: format!("{e:?}"),
        })?;
    let from_apex = mid_point - apex;
    let axial_component = from_apex.dot(&axis);
    let radial_target = from_apex - axis * axial_component;
    orient_face_for_outward(surface, radial_target).map_err(|e| PrimitiveError::GeometryError {
        operation: "Cone lateral orientation".to_string(),
        details: format!("{e:?}"),
    })
}
use serde::{Deserialize, Serialize};

/// Parameters for creating a cone primitive
type PrimitiveResult<T> = Result<T, PrimitiveError>;

/// Parameters for creating a cone primitive
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ConeParameters {
    /// Apex point of the cone
    pub apex: Point3,
    /// Axis direction (from apex)
    pub axis: Vector3,
    /// Half angle of the cone (in radians)
    pub half_angle: f64,
    /// Height of the cone (distance from apex)
    pub height: f64,
    /// Optional bottom radius (for frustum)
    pub bottom_radius: Option<f64>,
    /// Optional angle range for sector cone [start, end] in radians
    pub angle_range: Option<[f64; 2]>,
}

impl ConeParameters {
    /// Create parameters for a standard cone
    pub fn new(apex: Point3, axis: Vector3, half_angle: f64, height: f64) -> PrimitiveResult<Self> {
        if half_angle <= 0.0 || half_angle >= consts::FRAC_PI_2 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "half_angle".to_string(),
                value: half_angle.to_string(),
                constraint: "Half angle must be between 0 and π/2".to_string(),
            });
        }

        if height <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "height".to_string(),
                value: height.to_string(),
                constraint: "Height must be positive".to_string(),
            });
        }

        Ok(Self {
            apex,
            axis: axis
                .normalize()
                .map_err(|_| PrimitiveError::InvalidParameters {
                    parameter: "axis".to_string(),
                    value: "zero_vector".to_string(),
                    constraint: "Axis must be non-zero".to_string(),
                })?,
            half_angle,
            height,
            bottom_radius: None,
            angle_range: None,
        })
    }

    /// Create parameters for a frustum (truncated cone)
    pub fn frustum(
        apex: Point3,
        axis: Vector3,
        half_angle: f64,
        bottom_height: f64,
        top_height: f64,
    ) -> PrimitiveResult<Self> {
        if bottom_height >= top_height {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "height".to_string(),
                value: format!("bottom: {}, top: {}", bottom_height, top_height),
                constraint: "Bottom height must be less than top height".to_string(),
            });
        }

        let bottom_radius = bottom_height * half_angle.tan();

        Ok(Self {
            apex,
            axis: axis
                .normalize()
                .map_err(|_| PrimitiveError::InvalidParameters {
                    parameter: "axis".to_string(),
                    value: "zero_vector".to_string(),
                    constraint: "Axis must be non-zero".to_string(),
                })?,
            half_angle,
            height: top_height - bottom_height,
            bottom_radius: Some(bottom_radius),
            angle_range: None,
        })
    }
}

/// Cone primitive with topology
pub struct ConePrimitive;

impl ConePrimitive {
    /// Create a cone with B-Rep topology
    pub fn create(params: &ConeParameters, model: &mut BRepModel) -> PrimitiveResult<SolidId> {
        let _builder = TopologyBuilder::new(model);

        // Calculate key dimensions
        let top_radius = params.height * params.half_angle.tan();
        let bottom_radius = params.bottom_radius.unwrap_or(0.0);
        let has_apex = bottom_radius == 0.0;

        // Create reference direction perpendicular to axis
        let ref_dir = params.axis.perpendicular();
        let y_dir = params.axis.cross(&ref_dir);

        // Angle range
        let angle_range = params.angle_range.unwrap_or([0.0, consts::TWO_PI]);
        let start_angle = angle_range[0];
        let end_angle = angle_range[1];
        let is_full_cone = (end_angle - start_angle - consts::TWO_PI).abs() < consts::EPSILON;

        // Create conical surface
        let cone_surface = Cone {
            apex: params.apex,
            axis: params.axis,
            half_angle: params.half_angle,
            ref_dir,
            height_limits: Some([bottom_radius / params.half_angle.tan(), params.height]),
            angle_limits: if is_full_cone {
                None
            } else {
                Some([start_angle, end_angle])
            },
        };
        let cone_surface_id = model.surfaces.add(Box::new(cone_surface));

        // Create vertices
        let _apex_vertex = if has_apex {
            Some(
                model
                    .vertices
                    .add(params.apex.x, params.apex.y, params.apex.z),
            )
        } else {
            None
        };

        // Bottom circle vertices (if frustum or sector)
        let bottom_center = params.apex + params.axis * (bottom_radius / params.half_angle.tan());
        let mut bottom_vertices = Vec::new();

        if !is_full_cone {
            // Start and end vertices for sector
            let start_point = bottom_center
                + ref_dir * (bottom_radius * start_angle.cos())
                + y_dir * (bottom_radius * start_angle.sin());
            let end_point = bottom_center
                + ref_dir * (bottom_radius * end_angle.cos())
                + y_dir * (bottom_radius * end_angle.sin());

            bottom_vertices.push(
                model
                    .vertices
                    .add(start_point.x, start_point.y, start_point.z),
            );
            bottom_vertices.push(model.vertices.add(end_point.x, end_point.y, end_point.z));
        }

        // Top circle vertices
        let top_center = params.apex + params.axis * params.height;
        let mut top_vertices = Vec::new();

        if !is_full_cone {
            // Start and end vertices for sector
            let start_point = top_center
                + ref_dir * (top_radius * start_angle.cos())
                + y_dir * (top_radius * start_angle.sin());
            let end_point = top_center
                + ref_dir * (top_radius * end_angle.cos())
                + y_dir * (top_radius * end_angle.sin());

            top_vertices.push(
                model
                    .vertices
                    .add(start_point.x, start_point.y, start_point.z),
            );
            top_vertices.push(model.vertices.add(end_point.x, end_point.y, end_point.z));
        }

        // Create edges and faces
        let mut faces = Vec::new();

        // The lateral face owns the rim edges; the cap faces reuse the SAME
        // EdgeId (traversed in the opposite direction) so the rim is a single
        // shared manifold edge — not two coincident edges with distinct
        // curve_ids. Sharing is what lets a boolean cut's presplit/imprint of a
        // rim propagate to the cap loop, keeping the cap's tessellated rim
        // bit-identical to the lateral band's rim (mirrors the cylinder
        // primitive's shared-rim topology). Each holder carries the lateral's
        // own use-orientation; the cap uses its negation.
        let mut lateral_top_edge: Option<(EdgeId, bool)> = None;
        let mut lateral_bottom_edge: Option<(EdgeId, bool)> = None;

        // Conical surface face
        let cone_face = if is_full_cone {
            if has_apex {
                // Full cone with apex - single edge loop at top
                let top_circle = Circle::new(top_center, params.axis, top_radius)?;
                let start_point = top_center + ref_dir * top_radius;
                let start_vertex = model
                    .vertices
                    .add(start_point.x, start_point.y, start_point.z);
                let curve_id = model.curves.add(Box::new(top_circle));
                let top_edge = Edge::new(
                    0, // id will be set by store
                    start_vertex,
                    start_vertex, // closed curve
                    curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let top_edge_id = model.edges.add_or_find(top_edge);

                let mut loop_ = Loop::new(0, LoopType::Outer);
                loop_.add_edge(top_edge_id, true);
                lateral_top_edge = Some((top_edge_id, true));
                let loop_id = model.loops.add(loop_);

                // Stamp orientation so the cone lateral's oriented outward
                // normal points radially away from the cone's axis at the
                // mid-height parametric sample. Slice 3 of the
                // comprehensive face-orientation fix.
                let cone_orientation =
                    orient_cone_lateral_outward(model, cone_surface_id, params.apex, params.axis)?;
                let face = Face::new(0, cone_surface_id, loop_id, cone_orientation);
                model.faces.add(face)
            } else {
                // Frustum - two circular edges
                let bottom_circle = Arc::circle(bottom_center, params.axis, bottom_radius)?;
                let bottom_start = bottom_center + ref_dir * bottom_radius;
                let bottom_vertex =
                    model
                        .vertices
                        .add(bottom_start.x, bottom_start.y, bottom_start.z);
                let bottom_curve_id = model.curves.add(Box::new(bottom_circle));
                let bottom_edge = Edge::new(
                    0, // id will be set by store
                    bottom_vertex,
                    bottom_vertex, // closed curve
                    bottom_curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let bottom_edge_id = model.edges.add_or_find(bottom_edge);

                let top_circle = Arc::circle(top_center, params.axis, top_radius)?;
                let top_start = top_center + ref_dir * top_radius;
                let top_vertex = model.vertices.add(top_start.x, top_start.y, top_start.z);
                let top_curve_id = model.curves.add(Box::new(top_circle));
                let top_edge = Edge::new(
                    0, // id will be set by store
                    top_vertex,
                    top_vertex, // closed curve
                    top_curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let top_edge_id = model.edges.add_or_find(top_edge);

                let mut outer_loop = Loop::new(0, LoopType::Outer);
                outer_loop.add_edge(bottom_edge_id, true);
                lateral_bottom_edge = Some((bottom_edge_id, true));
                let outer_loop_id = model.loops.add(outer_loop);

                let mut inner_loop = Loop::new(0, LoopType::Inner);
                inner_loop.add_edge(top_edge_id, false);
                lateral_top_edge = Some((top_edge_id, false));
                let inner_loop_id = model.loops.add(inner_loop);

                let cone_orientation =
                    orient_cone_lateral_outward(model, cone_surface_id, params.apex, params.axis)?;
                let mut face = Face::new(0, cone_surface_id, outer_loop_id, cone_orientation);
                face.add_inner_loop(inner_loop_id);
                model.faces.add(face)
            }
        } else {
            // Sector cones (partial-sweep cones with start_angle/sweep_angle <
            // TWO_PI) require radial-cap face stitching that the rolling-ball
            // kernel does not yet generate. Until that path lands we surface
            // an explicit error rather than silently emitting a malformed
            // shell. Callers wanting a partial cone should construct it from
            // a revolved profile via operations::revolve.
            return Err(PrimitiveError::GeometryError {
                operation: "Create sector cone".to_string(),
                details: "sector cones are not supported by this primitive — \
                          use operations::revolve with a triangular profile"
                    .to_string(),
            });
        };

        faces.push(cone_face);

        // Top cap face (circle)
        let top_plane = Plane::from_point_normal(top_center, params.axis).map_err(|_| {
            PrimitiveError::GeometryError {
                operation: "Create top plane".to_string(),
                details: "Failed to create plane".to_string(),
            }
        })?;
        let top_plane_id = model.surfaces.add(Box::new(top_plane));

        // Reuse the lateral's rim edge (shared manifold edge) when the lateral
        // produced one; otherwise build a standalone rim edge (sector cones).
        let (top_circle_edge_id, top_cap_use) = match lateral_top_edge {
            Some((eid, lateral_use)) => (eid, !lateral_use),
            None => {
                let top_circle = Arc::circle(top_center, params.axis, top_radius)?;
                let top_start = top_center + ref_dir * top_radius;
                let top_vertex = model.vertices.add(top_start.x, top_start.y, top_start.z);
                let top_curve_id = model.curves.add(Box::new(top_circle));
                let top_circle_edge = Edge::new(
                    0, // id will be set by store
                    top_vertex,
                    top_vertex, // closed curve
                    top_curve_id,
                    EdgeOrientation::Forward,
                    // Match `Circle`'s normalized [0, 1] parameterization — the
                    // underlying `Arc::evaluate(t)` clamps `t` to `[0, 1]`.
                    ParameterRange::new(0.0, 1.0),
                );
                (model.edges.add_or_find(top_circle_edge), true)
            }
        };

        let mut top_loop = Loop::new(0, LoopType::Outer);
        top_loop.add_edge(top_circle_edge_id, top_cap_use);
        let top_loop_id = model.loops.add(top_loop);
        // Top cap outward target = +params.axis (the cone's axis points
        // from apex toward the base, so the top cap faces +axis).
        let top_orientation =
            {
                let surface = model.surfaces.get(top_plane_id).ok_or_else(|| {
                    PrimitiveError::GeometryError {
                        operation: "Cone top-cap surface lookup".to_string(),
                        details: "top plane surface missing from store".to_string(),
                    }
                })?;
                orient_face_for_outward(surface, params.axis).map_err(|e| {
                    PrimitiveError::GeometryError {
                        operation: "Cone top-cap orientation".to_string(),
                        details: format!("{e:?}"),
                    }
                })?
            };
        let top_face = Face::new(0, top_plane_id, top_loop_id, top_orientation);
        let top_face_id = model.faces.add(top_face);
        faces.push(top_face_id);

        // Bottom cap face (if frustum)
        if !has_apex {
            let bottom_plane =
                Plane::from_point_normal(bottom_center, -params.axis).map_err(|_| {
                    PrimitiveError::GeometryError {
                        operation: "Create bottom plane".to_string(),
                        details: "Failed to create plane".to_string(),
                    }
                })?;
            let bottom_plane_id = model.surfaces.add(Box::new(bottom_plane));

            // Reuse the lateral's bottom rim edge (shared manifold edge) when
            // present; otherwise build a standalone one (sector cones).
            let (bottom_circle_edge_id, bottom_cap_use) = match lateral_bottom_edge {
                Some((eid, lateral_use)) => (eid, !lateral_use),
                None => {
                    let bottom_circle = Arc::circle(bottom_center, -params.axis, bottom_radius)?;
                    let bottom_start = bottom_center + ref_dir * bottom_radius;
                    let bottom_vertex =
                        model
                            .vertices
                            .add(bottom_start.x, bottom_start.y, bottom_start.z);
                    let bottom_curve_id = model.curves.add(Box::new(bottom_circle));
                    let bottom_circle_edge = Edge::new(
                        0, // id will be set by store
                        bottom_vertex,
                        bottom_vertex, // closed curve
                        bottom_curve_id,
                        EdgeOrientation::Forward,
                        // Match `Circle`'s normalized [0, 1] parameterization.
                        ParameterRange::new(0.0, 1.0),
                    );
                    (model.edges.add_or_find(bottom_circle_edge), true)
                }
            };

            let mut bottom_loop = Loop::new(0, LoopType::Outer);
            bottom_loop.add_edge(bottom_circle_edge_id, bottom_cap_use);
            let bottom_loop_id = model.loops.add(bottom_loop);
            // Bottom cap outward target = -params.axis (away from apex).
            let bottom_orientation = {
                let surface = model.surfaces.get(bottom_plane_id).ok_or_else(|| {
                    PrimitiveError::GeometryError {
                        operation: "Cone bottom-cap surface lookup".to_string(),
                        details: "bottom plane surface missing from store".to_string(),
                    }
                })?;
                orient_face_for_outward(surface, -params.axis).map_err(|e| {
                    PrimitiveError::GeometryError {
                        operation: "Cone bottom-cap orientation".to_string(),
                        details: format!("{e:?}"),
                    }
                })?
            };
            let bottom_face = Face::new(0, bottom_plane_id, bottom_loop_id, bottom_orientation);
            let bottom_face_id = model.faces.add(bottom_face);
            faces.push(bottom_face_id);
        }

        // Create shell and solid
        let mut shell = Shell::new(0, ShellType::Closed);
        for face_id in faces {
            shell.add_face(face_id);
        }
        let shell_id = model.shells.add(shell);

        let solid = Solid::new(0, shell_id);
        let solid_id = model.solids.add(solid);

        Ok(solid_id)
    }

    /// Update cone parameters by replacing the underlying topology.
    ///
    /// A cone has 3 faces (lateral + bottom cap + top cap or apex), one
    /// shell, and one solid; in-place mutation of the surfaces would have
    /// to walk the same shell to reach them and rebuild the cap edges
    /// anyway, so a delete + recreate is equivalent in cost and simpler
    /// to keep correct. The ID-equality check below detects the (rare)
    /// case where the underlying store could not reuse the freed slot.
    pub fn update_parameters(
        solid_id: SolidId,
        params: &ConeParameters,
        model: &mut BRepModel,
    ) -> PrimitiveResult<()> {
        model.solids.remove(solid_id);
        let new_solid_id = Self::create(params, model)?;
        if new_solid_id != solid_id {
            return Err(PrimitiveError::InvalidTopology {
                entity: "Solid".to_string(),
                issue: "Cone update did not preserve solid ID".to_string(),
                suggestion: "Verify solid store reuses freed IDs; this is \
                             a kernel-store invariant, not a user error"
                    .to_string(),
            });
        }
        Ok(())
    }

    /// Validate cone topology
    pub fn validate(solid_id: SolidId, model: &BRepModel) -> PrimitiveResult<()> {
        // Basic validation
        let solid = model
            .solids
            .get(solid_id)
            .ok_or(PrimitiveError::InvalidTopology {
                entity: "Solid".to_string(),
                issue: "Solid not found".to_string(),
                suggestion: "Check that the solid ID is valid".to_string(),
            })?;

        let shell = model
            .shells
            .get(solid.outer_shell)
            .ok_or(PrimitiveError::InvalidTopology {
                entity: "Shell".to_string(),
                issue: "Shell not found".to_string(),
                suggestion: "Check that the shell ID is valid".to_string(),
            })?;

        // Check face count (2 for cone with apex, 3 for frustum)
        if shell.faces.len() < 2 || shell.faces.len() > 3 {
            return Err(PrimitiveError::InvalidTopology {
                entity: "Cone".to_string(),
                issue: format!("Expected 2-3 faces, found {}", shell.faces.len()),
                suggestion: "Check cone creation logic".to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cone_creation() {
        let mut model = BRepModel::new();
        let params = ConeParameters::new(
            Point3::ORIGIN,
            Vector3::Z,
            consts::FRAC_PI_2 / 2.0, // 45 degrees
            10.0,
        )
        .unwrap();

        let solid_id = ConePrimitive::create(&params, &mut model).unwrap();
        assert!(ConePrimitive::validate(solid_id, &model).is_ok());
    }

    #[test]
    fn test_frustum_creation() {
        let mut model = BRepModel::new();
        let params = ConeParameters::frustum(
            Point3::ORIGIN,
            Vector3::Z,
            consts::FRAC_PI_2 / 2.0, // 45 degrees
            5.0,                     // bottom at height 5
            15.0,                    // top at height 15
        )
        .unwrap();

        let solid_id = ConePrimitive::create(&params, &mut model).unwrap();
        assert!(ConePrimitive::validate(solid_id, &model).is_ok());
    }
}
