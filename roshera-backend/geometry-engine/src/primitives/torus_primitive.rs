//! Torus primitive implementation
//!
//! This module provides a production-grade torus primitive with full B-Rep topology generation.
//! Handles self-intersection cases, partial tori, and proper UV parameterization.
//!
//! Indexed access into torus seam-edge / vertex arrays is the canonical idiom —
//! bounded by topology constants. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use crate::{
    math::{consts, Point3, Vector3},
    primitives::{
        curve::{Arc, Circle, ParameterRange},
        edge::{Edge, EdgeOrientation},
        face::{Face, FaceOrientation},
        primitive_traits::PrimitiveError,
        r#loop::{Loop, LoopType},
        shell::{Shell, ShellType},
        solid::{Solid, SolidId},
        surface::Torus,
        topology_builder::BRepModel,
    },
};
use serde::{Deserialize, Serialize};

type PrimitiveResult<T> = Result<T, PrimitiveError>;

/// Parameters for creating a torus primitive
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct TorusParameters {
    /// Center point of the torus
    pub center: Point3,
    /// Axis direction (rotation axis)
    pub axis: Vector3,
    /// Major radius (from center to tube center)
    pub major_radius: f64,
    /// Minor radius (tube radius)
    pub minor_radius: f64,
    /// Optional angle range around major circle [start, end] in radians
    pub major_angle_range: Option<[f64; 2]>,
    /// Optional angle range around minor circle [start, end] in radians
    pub minor_angle_range: Option<[f64; 2]>,
}

impl TorusParameters {
    /// Create parameters for a standard full torus
    pub fn new(
        center: Point3,
        axis: Vector3,
        major_radius: f64,
        minor_radius: f64,
    ) -> PrimitiveResult<Self> {
        if major_radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "major_radius".to_string(),
                value: major_radius.to_string(),
                constraint: "Major radius must be positive".to_string(),
            });
        }

        if minor_radius <= 0.0 {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "minor_radius".to_string(),
                value: minor_radius.to_string(),
                constraint: "Minor radius must be positive".to_string(),
            });
        }

        if minor_radius >= major_radius {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "minor_radius".to_string(),
                value: minor_radius.to_string(),
                constraint:
                    "Minor radius must be less than major radius for non-self-intersecting torus"
                        .to_string(),
            });
        }

        Ok(Self {
            center,
            axis: axis
                .normalize()
                .map_err(|_| PrimitiveError::InvalidParameters {
                    parameter: "axis".to_string(),
                    value: "zero_vector".to_string(),
                    constraint: "Axis must be non-zero".to_string(),
                })?,
            major_radius,
            minor_radius,
            major_angle_range: None,
            minor_angle_range: None,
        })
    }

    /// Create parameters for a partial torus (sector)
    pub fn partial(
        center: Point3,
        axis: Vector3,
        major_radius: f64,
        minor_radius: f64,
        major_angles: [f64; 2],
        minor_angles: [f64; 2],
    ) -> PrimitiveResult<Self> {
        let mut params = Self::new(center, axis, major_radius, minor_radius)?;

        if major_angles[0] >= major_angles[1] {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "major_angles".to_string(),
                value: format!("[{}, {}]", major_angles[0], major_angles[1]),
                constraint: "Start angle must be less than end angle".to_string(),
            });
        }

        if minor_angles[0] >= minor_angles[1] {
            return Err(PrimitiveError::InvalidParameters {
                parameter: "minor_angles".to_string(),
                value: format!("[{}, {}]", minor_angles[0], minor_angles[1]),
                constraint: "Start angle must be less than end angle".to_string(),
            });
        }

        params.major_angle_range = Some(major_angles);
        params.minor_angle_range = Some(minor_angles);

        Ok(params)
    }
}

/// Torus primitive with topology
pub struct TorusPrimitive;

impl TorusPrimitive {
    /// Create a torus with B-Rep topology
    pub fn create(params: &TorusParameters, model: &mut BRepModel) -> PrimitiveResult<SolidId> {
        // Topology is assembled directly against `model` below; the
        // TopologyBuilder helper is not needed for this primitive's seam
        // loop / single face layout. The reference frame (`ref_dir`,
        // implicit y_dir) is recomputed once the axis has been normalised
        // a few lines down — keeping a duplicate copy here would just
        // diverge if the axis fails normalisation.

        // Angle ranges
        let major_angle_range = params.major_angle_range.unwrap_or([0.0, consts::TWO_PI]);
        let u_start = major_angle_range[0];
        let u_end = major_angle_range[1];
        let minor_angle_range = params.minor_angle_range.unwrap_or([0.0, consts::TWO_PI]);
        let v_start = minor_angle_range[0];
        let v_end = minor_angle_range[1];
        let is_closed_u = (u_end - u_start - consts::TWO_PI).abs() < consts::EPSILON;
        let is_closed_v = (v_end - v_start - consts::TWO_PI).abs() < consts::EPSILON;

        // Create torus surface
        let ref_dir =
            params
                .axis
                .perpendicular()
                .normalize()
                .map_err(|_| PrimitiveError::GeometryError {
                    operation: "Create torus surface".to_string(),
                    details: "Failed to find perpendicular to axis".to_string(),
                })?;

        let mut torus_surface = Torus {
            center: params.center,
            axis: params
                .axis
                .normalize()
                .map_err(|_| PrimitiveError::GeometryError {
                    operation: "Create torus surface".to_string(),
                    details: "Invalid axis vector".to_string(),
                })?,
            major_radius: params.major_radius,
            minor_radius: params.minor_radius,
            ref_dir,
            param_limits: None,
        };

        if !is_closed_u || !is_closed_v {
            torus_surface.param_limits = Some([u_start, u_end, v_start, v_end]);
        }

        let torus_surface_id = model.surfaces.add(Box::new(torus_surface));

        // For a full torus, create proper minimal cell complex topology.
        // A torus is a genus-1 surface with Euler characteristic χ = V - E + F = 0.
        // Minimal cell complex: 1 vertex, 2 edges (seam loops), 1 face.
        // Both edges are loops from the single vertex back to itself:
        //   - Edge 1 (u-seam): follows constant v=0 around the major circle
        //   - Edge 2 (v-seam): follows constant u=0 around the minor circle
        let torus_face = if is_closed_u && is_closed_v {
            // Single vertex at (u=0, v=0)
            let p = Self::evaluate_point(params, 0.0, 0.0);
            let v_id = model.vertices.add(p.x, p.y, p.z);

            // Edge 1: u-seam (major circle at v=0), loops from v_id back to v_id
            let edge1_curve = Self::create_major_circle_arc(params, 0.0, 0.0, consts::TWO_PI);
            let edge1_curve_id = model.curves.add(edge1_curve);
            let edge1 = Edge::new(
                0,
                v_id,
                v_id, // loop edge: same start and end vertex
                edge1_curve_id,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            );
            // Use .add() directly — both seam edges share the same vertex pair (v_id, v_id)
            // so add_or_find() would incorrectly deduplicate them.
            let edge1_id = model.edges.add(edge1);

            // Edge 2: v-seam (minor circle at u=0), loops from v_id back to v_id
            let edge2_curve = Self::create_minor_circle_arc(params, 0.0, 0.0, consts::TWO_PI);
            let edge2_curve_id = model.curves.add(edge2_curve);
            let edge2 = Edge::new(
                0,
                v_id,
                v_id, // loop edge: same start and end vertex
                edge2_curve_id,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            );
            let edge2_id = model.edges.add(edge2);

            // Create loop with both seam edges
            // V=1, E=2, F=1 → χ = 1 - 2 + 1 = 0 ✓ (genus-1 torus)
            let mut outer_loop = Loop::new(0, LoopType::Outer);
            outer_loop.add_edge(edge1_id, true);
            outer_loop.add_edge(edge2_id, true);
            let outer_loop_id = model.loops.add(outer_loop);

            let face = Face::new(0, torus_surface_id, outer_loop_id, FaceOrientation::Forward);
            model.faces.add(face)
        } else {
            // Partial torus - create boundary edges
            let mut edges = Vec::new();
            let mut orientations = Vec::new();

            // Create vertices at corners
            let v00 = Self::evaluate_point(params, u_start, v_start);
            let v10 = Self::evaluate_point(params, u_end, v_start);
            let v01 = Self::evaluate_point(params, u_start, v_end);
            let v11 = Self::evaluate_point(params, u_end, v_end);

            let v00_id = model.vertices.add(v00.x, v00.y, v00.z);
            let v10_id = model.vertices.add(v10.x, v10.y, v10.z);
            let v01_id = model.vertices.add(v01.x, v01.y, v01.z);
            let v11_id = model.vertices.add(v11.x, v11.y, v11.z);

            // Bottom edge (v = v_start)
            if !is_closed_u {
                let bottom_curve = Self::create_major_circle_arc(params, v_start, u_start, u_end);
                let bottom_curve_id = model.curves.add(bottom_curve);
                let bottom_edge = Edge::new(
                    0, // id will be set by store
                    v00_id,
                    v10_id,
                    bottom_curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let bottom_edge_id = model.edges.add_or_find(bottom_edge);
                edges.push(bottom_edge_id);
                orientations.push(true);
            }

            // Right edge (u = u_end)
            if !is_closed_v {
                let right_curve = Self::create_minor_circle_arc(params, u_end, v_start, v_end);
                let right_curve_id = model.curves.add(right_curve);
                let right_edge = Edge::new(
                    0, // id will be set by store
                    v10_id,
                    v11_id,
                    right_curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let right_edge_id = model.edges.add_or_find(right_edge);
                edges.push(right_edge_id);
                orientations.push(true);
            }

            // Top edge (v = v_end)
            if !is_closed_u {
                let top_curve = Self::create_major_circle_arc(params, v_end, u_end, u_start);
                let top_curve_id = model.curves.add(top_curve);
                let top_edge = Edge::new(
                    0, // id will be set by store
                    v11_id,
                    v01_id,
                    top_curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let top_edge_id = model.edges.add_or_find(top_edge);
                edges.push(top_edge_id);
                orientations.push(true);
            }

            // Left edge (u = u_start)
            if !is_closed_v {
                let left_curve = Self::create_minor_circle_arc(params, u_start, v_end, v_start);
                let left_curve_id = model.curves.add(left_curve);
                let left_edge = Edge::new(
                    0, // id will be set by store
                    v01_id,
                    v00_id,
                    left_curve_id,
                    EdgeOrientation::Forward,
                    ParameterRange::new(0.0, 1.0),
                );
                let left_edge_id = model.edges.add_or_find(left_edge);
                edges.push(left_edge_id);
                orientations.push(true);
            }

            let mut boundary_loop = Loop::new(0, LoopType::Outer);
            for (i, edge_id) in edges.iter().enumerate() {
                boundary_loop.add_edge(*edge_id, orientations[i]);
            }
            let boundary_loop_id = model.loops.add(boundary_loop);
            let face = Face::new(
                0,
                torus_surface_id,
                boundary_loop_id,
                FaceOrientation::Forward,
            );
            model.faces.add(face)
        };

        // Create shell and solid
        let mut shell = Shell::new(0, ShellType::Closed);
        shell.add_face(torus_face);
        let shell_id = model.shells.add(shell);

        let solid = Solid::new(0, shell_id);
        let solid_id = model.solids.add(solid);

        Ok(solid_id)
    }

    /// Evaluate a point on the torus
    fn evaluate_point(params: &TorusParameters, u: f64, v: f64) -> Point3 {
        let (sin_u, cos_u) = u.sin_cos();
        let (sin_v, cos_v) = v.sin_cos();

        let ref_dir = params.axis.perpendicular();
        let y_dir = params.axis.cross(&ref_dir);

        let major_point = params.center
            + ref_dir * (params.major_radius * cos_u)
            + y_dir * (params.major_radius * sin_u);

        let radial_dir = ref_dir * cos_u + y_dir * sin_u;
        

        major_point
            + radial_dir * (params.minor_radius * cos_v)
            + params.axis * (params.minor_radius * sin_v)
    }

    /// Create arc along major circle
    #[allow(clippy::expect_used)] // Circle/Arc inputs derived from validated TorusParameters
    fn create_major_circle_arc(
        params: &TorusParameters,
        v: f64,
        u_start: f64,
        u_end: f64,
    ) -> Box<dyn crate::primitives::curve::Curve> {
        // For v parameter, create arc at distance from center
        let (sin_v, cos_v) = v.sin_cos();
        let radius = params.major_radius + params.minor_radius * cos_v;
        let height = params.minor_radius * sin_v;

        let center = params.center + params.axis * height;
        // Circle::new / Arc::new derive their own in-plane reference direction
        // from the supplied axis, so we don't need to materialise a separate
        // ref_dir here — the major circle is fully determined by
        // (center, axis, radius).

        if (u_end - u_start - consts::TWO_PI).abs() < consts::EPSILON {
            Box::new(
                Circle::new(center, params.axis, radius)
                    .expect("Circle inputs derived from validated TorusParameters"),
            )
        } else {
            Box::new(
                Arc::new(center, params.axis, radius, u_start, u_end - u_start)
                    .expect("Arc inputs derived from validated TorusParameters"),
            )
        }
    }

    /// Create arc along minor circle
    #[allow(clippy::expect_used)] // Circle/Arc inputs derived from validated TorusParameters
    fn create_minor_circle_arc(
        params: &TorusParameters,
        u: f64,
        v_start: f64,
        v_end: f64,
    ) -> Box<dyn crate::primitives::curve::Curve> {
        let (sin_u, cos_u) = u.sin_cos();

        let ref_dir = params.axis.perpendicular();
        let y_dir = params.axis.cross(&ref_dir);

        let center = params.center
            + ref_dir * (params.major_radius * cos_u)
            + y_dir * (params.major_radius * sin_u);

        // The minor circle lies in the tube cross-section plane spanned
        // by (radial_dir, axis). Its plane normal is therefore
        // `radial_dir × axis`, NOT `axis` itself — using axis here puts
        // the minor circle in the same horizontal plane as the major
        // circle, collapsing the tube to a flat ring. The tessellator
        // then sees the boundary edge tracing only z=0 samples, lifts
        // those to v ∈ {0, π}, and renders only the top half of the
        // torus ("sliced bagel" bug).
        let radial_dir = ref_dir * cos_u + y_dir * sin_u;
        // radial_dir lies in the (ref_dir, y_dir) plane which is
        // perpendicular to axis, so |radial_dir × axis| = 1 and
        // normalize() cannot fail here.
        #[allow(clippy::expect_used)]
        // Reason: radial_dir ⟂ axis by construction (radial_dir is a
        // unit vector in the plane perpendicular to axis), so the
        // cross product has unit length and normalize() never errs.
        let minor_axis = radial_dir
            .cross(&params.axis)
            .normalize()
            .expect("radial_dir ⟂ axis ⇒ |cross| = 1, normalize cannot fail");

        if (v_end - v_start - consts::TWO_PI).abs() < consts::EPSILON {
            Box::new(
                Circle::new(center, minor_axis, params.minor_radius)
                    .expect("Circle inputs derived from validated TorusParameters"),
            )
        } else {
            Box::new(
                Arc::new(
                    center,
                    minor_axis,
                    params.minor_radius,
                    v_start,
                    v_end - v_start,
                )
                .expect("Arc inputs derived from validated TorusParameters"),
            )
        }
    }

    /// Update torus parameters in place.
    ///
    /// Parametric update for a torus would tear down the old topology
    /// (single face with two seam edge loops, see [`Self::create`]) and
    /// rebuild against the new (`major_radius`, `minor_radius`, axis,
    /// centre, angle ranges). That rebuild path is not yet wired here —
    /// callers should delete and recreate the solid for now. We still
    /// validate that the target exists and surface the rejected request
    /// with concrete identifiers so log readers can correlate the call.
    pub fn update_parameters(
        solid_id: SolidId,
        params: &TorusParameters,
        model: &mut BRepModel,
    ) -> PrimitiveResult<()> {
        if model.solids.get(solid_id).is_none() {
            return Err(PrimitiveError::InvalidTopology {
                entity: "Solid".to_string(),
                issue: format!("Torus solid {} not found", solid_id),
                suggestion: "Verify the solid id before requesting an update"
                    .to_string(),
            });
        }
        Err(PrimitiveError::GeometryError {
            operation: "update_parameters".to_string(),
            details: format!(
                "Torus parametric update not yet implemented (solid {}, major_radius={}, minor_radius={})",
                solid_id, params.major_radius, params.minor_radius
            ),
        })
    }

    /// Validate torus topology
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

        // Torus should have exactly one face
        if shell.faces.len() != 1 {
            return Err(PrimitiveError::InvalidTopology {
                entity: "Torus".to_string(),
                issue: format!("Expected 1 face, found {}", shell.faces.len()),
                suggestion: "Check torus creation logic".to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_torus_creation() {
        let mut model = BRepModel::new();
        let params = TorusParameters::new(
            Point3::ORIGIN,
            Vector3::Z,
            10.0, // major radius
            3.0,  // minor radius
        )
        .unwrap();

        let solid_id = TorusPrimitive::create(&params, &mut model).unwrap();
        assert!(TorusPrimitive::validate(solid_id, &model).is_ok());
    }

    #[test]
    fn test_partial_torus_creation() {
        let mut model = BRepModel::new();
        let params = TorusParameters::partial(
            Point3::ORIGIN,
            Vector3::Z,
            10.0,              // major radius
            3.0,               // minor radius
            [0.0, consts::PI], // half torus around major
            [0.0, consts::PI], // half torus around minor
        )
        .unwrap();

        let solid_id = TorusPrimitive::create(&params, &mut model).unwrap();
        assert!(TorusPrimitive::validate(solid_id, &model).is_ok());
    }

    #[test]
    fn test_invalid_radii() {
        let result = TorusParameters::new(
            Point3::ORIGIN,
            Vector3::Z,
            5.0,  // major radius
            10.0, // minor radius > major - invalid!
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_torus_euler_characteristic() {
        let mut model = BRepModel::new();
        let params = TorusParameters::new(Point3::ORIGIN, Vector3::Z, 10.0, 3.0).unwrap();

        let solid_id = TorusPrimitive::create(&params, &mut model).unwrap();
        let solid = model.solids.get(solid_id).unwrap();
        let shell = model.shells.get(solid.outer_shell).unwrap();

        // Count V, E, F
        let f = shell.faces.len();
        let mut all_edges = std::collections::HashSet::new();
        let mut all_vertices = std::collections::HashSet::new();

        for &face_id in &shell.faces {
            if let Some(face) = model.faces.get(face_id) {
                if let Some(loop_data) = model.loops.get(face.outer_loop) {
                    for &edge_id in &loop_data.edges {
                        all_edges.insert(edge_id);
                        if let Some(edge) = model.edges.get(edge_id) {
                            all_vertices.insert(edge.start_vertex);
                            all_vertices.insert(edge.end_vertex);
                        }
                    }
                }
            }
        }

        let v = all_vertices.len();
        let e = all_edges.len();
        let euler = v as i32 - e as i32 + f as i32;

        assert_eq!(
            euler, 0,
            "Torus Euler characteristic should be 0 (genus-1), got V={v} - E={e} + F={f} = {euler}"
        );
    }
}
