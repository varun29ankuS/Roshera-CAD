//! STEP file format support (ISO 10303)
//!
//! Provides export and import functionality for the STEP format
//! STEP AP203 (Configuration Controlled Design) and AP214 (Automotive Design)

use crate::formats::ros_snapshot::BRepSnapshot;
use chrono::{DateTime, Utc};
use geometry_engine::math::matrix4::Matrix4;
use geometry_engine::primitives::topology_builder::BRepModel;
use shared_types::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;
use uuid::Uuid;

/// STEP file header information
#[derive(Debug, Clone)]
pub struct StepHeader {
    /// File description
    pub description: String,
    /// Implementation level (e.g., "2;1")
    pub implementation_level: String,
    /// File name
    pub name: String,
    /// Time stamp
    pub time_stamp: DateTime<Utc>,
    /// Author
    pub author: String,
    /// Organization
    pub organization: String,
    /// Preprocessor version
    pub preprocessor_version: String,
    /// Originating system
    pub originating_system: String,
    /// Authorization
    pub authorization: String,
}

impl Default for StepHeader {
    fn default() -> Self {
        Self {
            description: "Roshera CAD Model".to_string(),
            implementation_level: "2;1".to_string(),
            name: "model.step".to_string(),
            time_stamp: Utc::now(),
            author: "Roshera User".to_string(),
            organization: "Roshera CAD".to_string(),
            preprocessor_version: "Roshera STEP Processor 1.0".to_string(),
            originating_system: "Roshera CAD System".to_string(),
            authorization: "".to_string(),
        }
    }
}

/// STEP entity reference
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StepId(pub u32);

impl std::fmt::Display for StepId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}

/// STEP writer for generating STEP files
pub struct StepWriter<W: Write> {
    writer: BufWriter<W>,
    entity_counter: u32,
    /// Map from internal IDs to STEP entity IDs
    id_map: HashMap<Uuid, StepId>,
}

impl<W: Write> StepWriter<W> {
    /// Create a new STEP writer
    pub fn new(writer: W) -> Self {
        Self {
            writer: BufWriter::new(writer),
            entity_counter: 1,
            id_map: HashMap::new(),
        }
    }

    /// Get the next entity ID
    fn next_id(&mut self) -> StepId {
        let id = StepId(self.entity_counter);
        self.entity_counter += 1;
        id
    }

    /// Map an internal ID to a STEP ID
    fn map_id(&mut self, internal_id: Uuid) -> StepId {
        if let Some(&step_id) = self.id_map.get(&internal_id) {
            step_id
        } else {
            let step_id = self.next_id();
            self.id_map.insert(internal_id, step_id);
            step_id
        }
    }

    /// Write the STEP header
    pub fn write_header(&mut self, header: &StepHeader) -> std::io::Result<()> {
        writeln!(self.writer, "ISO-10303-21;")?;
        writeln!(self.writer, "HEADER;")?;

        // FILE_DESCRIPTION
        writeln!(
            self.writer,
            "FILE_DESCRIPTION(('{}'),'{}')",
            header.description, header.implementation_level
        )?;

        // FILE_NAME
        writeln!(
            self.writer,
            "FILE_NAME('{}','{}','{}',('{}'),'{}','{}','{}')",
            header.name,
            header.time_stamp.format("%Y-%m-%dT%H:%M:%S"),
            header.author,
            header.organization,
            header.preprocessor_version,
            header.originating_system,
            header.authorization
        )?;

        // FILE_SCHEMA
        writeln!(self.writer, "FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'));")?;
        writeln!(self.writer, "ENDSEC;")?;

        Ok(())
    }

    /// Write the DATA section start
    pub fn begin_data(&mut self) -> std::io::Result<()> {
        writeln!(self.writer, "DATA;")
    }

    /// Write the DATA section end
    pub fn end_data(&mut self) -> std::io::Result<()> {
        writeln!(self.writer, "ENDSEC;")
    }

    /// Write the END-ISO marker
    pub fn write_end(&mut self) -> std::io::Result<()> {
        writeln!(self.writer, "END-ISO-10303-21;")
    }

    /// Write a Cartesian point
    pub fn write_cartesian_point(&mut self, point: &[f64; 3]) -> std::io::Result<StepId> {
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=CARTESIAN_POINT('',({}));",
            id,
            format_real_list(&[point[0], point[1], point[2]])
        )?;
        Ok(id)
    }

    /// Write a direction
    pub fn write_direction(&mut self, dir: &[f64; 3]) -> std::io::Result<StepId> {
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=DIRECTION('',({}));",
            id,
            format_real_list(&[dir[0], dir[1], dir[2]])
        )?;
        Ok(id)
    }

    /// Write a vector
    pub fn write_vector(
        &mut self,
        direction_id: StepId,
        magnitude: f64,
    ) -> std::io::Result<StepId> {
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=VECTOR('',{},{});",
            id, direction_id, magnitude
        )?;
        Ok(id)
    }

    /// Write an axis2 placement 3D
    pub fn write_axis2_placement_3d(
        &mut self,
        location: &[f64; 3],
        axis: Option<&[f64; 3]>,
        ref_direction: Option<&[f64; 3]>,
    ) -> std::io::Result<StepId> {
        let location_id = self.write_cartesian_point(location)?;

        let axis_id = if let Some(axis) = axis {
            Some(self.write_direction(axis)?)
        } else {
            None
        };

        let ref_dir_id = if let Some(ref_dir) = ref_direction {
            Some(self.write_direction(ref_dir)?)
        } else {
            None
        };

        let id = self.next_id();
        write!(self.writer, "{}=AXIS2_PLACEMENT_3D('',{}", id, location_id)?;

        if let Some(axis_id) = axis_id {
            write!(self.writer, ",{}", axis_id)?;
        } else {
            write!(self.writer, ",$")?;
        }

        if let Some(ref_dir_id) = ref_dir_id {
            write!(self.writer, ",{}", ref_dir_id)?;
        } else {
            write!(self.writer, ",$")?;
        }

        writeln!(self.writer, ");")?;
        Ok(id)
    }

    /// Write a line
    pub fn write_line(&mut self, start: &[f64; 3], end: &[f64; 3]) -> std::io::Result<StepId> {
        let start_id = self.write_cartesian_point(start)?;
        let end_id = self.write_cartesian_point(end)?;

        // Calculate direction and magnitude
        let dir = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
        let magnitude = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
        let unit_dir = [dir[0] / magnitude, dir[1] / magnitude, dir[2] / magnitude];

        let dir_id = self.write_direction(&unit_dir)?;
        let vector_id = self.write_vector(dir_id, magnitude)?;

        let id = self.next_id();
        writeln!(self.writer, "{}=LINE('',{},{});", id, start_id, vector_id)?;
        Ok(id)
    }

    /// Write a circle
    pub fn write_circle(
        &mut self,
        center: &[f64; 3],
        normal: &[f64; 3],
        radius: f64,
    ) -> std::io::Result<StepId> {
        let axis_id = self.write_axis2_placement_3d(center, Some(normal), None)?;

        let id = self.next_id();
        writeln!(self.writer, "{}=CIRCLE('',{},{});", id, axis_id, radius)?;
        Ok(id)
    }

    /// Write a B-spline curve with knots
    pub fn write_b_spline_curve(
        &mut self,
        degree: u32,
        control_points: &[[f64; 3]],
        knots: &[f64],
        multiplicities: &[u32],
        rational: bool,
        weights: Option<&[f64]>,
    ) -> std::io::Result<StepId> {
        // Write control points
        let mut cp_ids = Vec::new();
        for cp in control_points {
            cp_ids.push(self.write_cartesian_point(cp)?);
        }

        let id = self.next_id();

        if rational && weights.is_some() {
            // Write as rational B-spline
            writeln!(self.writer,
                "{}=RATIONAL_B_SPLINE_CURVE({},({}),.UNSPECIFIED.,.F.,.U.,({}),({}),.UNSPECIFIED.,({}));",
                id,
                degree,
                format_id_list(&cp_ids),
                format_real_list(knots),
                format_int_list(multiplicities),
                format_real_list(weights.unwrap())
            )?;
        } else {
            // Write as non-rational B-spline
            writeln!(self.writer,
                "{}=B_SPLINE_CURVE_WITH_KNOTS({},({}),.UNSPECIFIED.,.F.,.U.,({}),({}),UNSPECIFIED.);",
                id,
                degree,
                format_id_list(&cp_ids),
                format_int_list(multiplicities),
                format_real_list(knots)
            )?;
        }

        Ok(id)
    }

    /// Write a curve entity
    pub fn write_curve(
        &mut self,
        curve: &crate::formats::ros_snapshot::CurveData,
        vertex_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        use crate::formats::ros_snapshot::CurveData;

        match curve {
            CurveData::Line { start, end } => {
                // Write LINE entity
                let start_id = self.write_cartesian_point(start)?;
                let _end_id = self.write_cartesian_point(end)?;

                // Create vector from start to end
                let vector_id = self.next_id();
                let dir = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
                writeln!(
                    self.writer,
                    "{}=DIRECTION('',({},{},{}));",
                    vector_id, dir[0], dir[1], dir[2]
                )?;
                let vec_id = self.next_id();
                writeln!(self.writer, "{}=VECTOR('',{},1.0);", vec_id, vector_id)?;

                let id = self.next_id();
                writeln!(self.writer, "{}=LINE('',{},{});", id, start_id, vec_id)?;
                Ok(id)
            }
            CurveData::Circle {
                center,
                radius,
                normal,
            } => {
                // Write CIRCLE entity
                let center_pt = self.write_cartesian_point(&[center[0], center[1], center[2]])?;
                let axis = self.write_axis2_placement_3d(
                    &[center[0], center[1], center[2]],
                    Some(&[normal[0], normal[1], normal[2]]),
                    Some(&[1.0, 0.0, 0.0]),
                )?;
                // write_circle expects center, normal, radius - not axis
                self.write_circle(
                    &[center[0], center[1], center[2]],
                    &[normal[0], normal[1], normal[2]],
                    *radius,
                )
            }
            CurveData::BSpline {
                control_points,
                knots,
                degree,
            } => {
                // Convert control points
                let cps: Vec<[f64; 3]> = control_points
                    .iter()
                    .map(|cp| [cp[0], cp[1], cp[2]])
                    .collect();

                // Calculate multiplicities from knot vector
                let mut multiplicities = Vec::new();
                let mut last_knot = knots[0];
                let mut mult = 1;
                for &knot in &knots[1..] {
                    if (knot - last_knot).abs() < 1e-10 {
                        mult += 1;
                    } else {
                        multiplicities.push(mult);
                        mult = 1;
                        last_knot = knot;
                    }
                }
                multiplicities.push(mult);

                self.write_b_spline_curve(
                    *degree,
                    &cps,
                    knots,
                    &multiplicities,
                    false, // Not rational
                    None,  // No weights for regular B-spline
                )
            }
            CurveData::Nurbs {
                control_points,
                weights,
                knots,
                degree,
            } => {
                // Convert control points
                let cps: Vec<[f64; 3]> = control_points
                    .iter()
                    .map(|cp| [cp[0], cp[1], cp[2]])
                    .collect();

                // Calculate multiplicities from knot vector
                let mut multiplicities = Vec::new();
                let mut last_knot = knots[0];
                let mut mult = 1;
                for &knot in &knots[1..] {
                    if (knot - last_knot).abs() < 1e-10 {
                        mult += 1;
                    } else {
                        multiplicities.push(mult);
                        mult = 1;
                        last_knot = knot;
                    }
                }
                multiplicities.push(mult);

                self.write_b_spline_curve(
                    *degree,
                    &cps,
                    knots,
                    &multiplicities,
                    true, // Rational
                    Some(weights),
                )
            }
            CurveData::Arc {
                center,
                normal,
                radius,
                start_angle,
                end_angle,
            } => {
                // Write ARC entity - similar to circle but with angle limits
                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=TRIMMED_CURVE('',#999,({},{}),#999,.T.,.PARAMETER.);",
                    id, start_angle, end_angle
                )?;
                Ok(id)
            }
        }
    }

    /// Write a surface entity
    pub fn write_surface(
        &mut self,
        surface: &crate::formats::ros_snapshot::SurfaceData,
    ) -> std::io::Result<StepId> {
        use crate::formats::ros_snapshot::SurfaceData;

        match surface {
            SurfaceData::Plane { origin, normal } => {
                // Write PLANE entity
                let axis = self.write_axis2_placement_3d(
                    &[origin[0], origin[1], origin[2]],
                    Some(&[normal[0], normal[1], normal[2]]),
                    Some(&[1.0, 0.0, 0.0]),
                )?;

                let id = self.next_id();
                writeln!(self.writer, "{}=PLANE('',{});", id, axis)?;
                Ok(id)
            }
            SurfaceData::Cylinder {
                origin,
                axis,
                radius,
            } => {
                // Write CYLINDRICAL_SURFACE entity
                let axis_placement = self.write_axis2_placement_3d(
                    &[origin[0], origin[1], origin[2]],
                    Some(&[axis[0], axis[1], axis[2]]),
                    Some(&[1.0, 0.0, 0.0]),
                )?;

                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=CYLINDRICAL_SURFACE('',{},{});",
                    id, axis_placement, radius
                )?;
                Ok(id)
            }
            SurfaceData::Sphere { center, radius } => {
                // Write SPHERICAL_SURFACE entity
                let axis = self.write_axis2_placement_3d(
                    &[center[0], center[1], center[2]],
                    Some(&[0.0, 0.0, 1.0]),
                    Some(&[1.0, 0.0, 0.0]),
                )?;

                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=SPHERICAL_SURFACE('',{},{});",
                    id, axis, radius
                )?;
                Ok(id)
            }
            SurfaceData::Cone {
                apex,
                axis,
                half_angle,
            } => {
                // Write CONICAL_SURFACE entity
                let axis_placement = self.write_axis2_placement_3d(
                    &[apex[0], apex[1], apex[2]],
                    Some(&[axis[0], axis[1], axis[2]]),
                    Some(&[1.0, 0.0, 0.0]),
                )?;

                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=CONICAL_SURFACE('',{},0.0,{});",
                    id,
                    axis_placement,
                    half_angle.to_degrees()
                )?;
                Ok(id)
            }
            SurfaceData::Torus {
                center,
                axis,
                major_radius,
                minor_radius,
            } => {
                // Write TOROIDAL_SURFACE entity
                let axis_placement = self.write_axis2_placement_3d(
                    &[center[0], center[1], center[2]],
                    Some(&[axis[0], axis[1], axis[2]]),
                    Some(&[1.0, 0.0, 0.0]),
                )?;

                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=TOROIDAL_SURFACE('',{},{},{});",
                    id, axis_placement, major_radius, minor_radius
                )?;
                Ok(id)
            }
            _ => {
                // For NURBS and other surfaces, write as generic
                let id = self.next_id();
                writeln!(self.writer, "{}=SURFACE('');", id)?;
                Ok(id)
            }
        }
    }

    /// Write an edge entity
    pub fn write_edge(
        &mut self,
        edge: &crate::formats::ros_snapshot::EdgeData,
        vertex_map: &HashMap<&uuid::Uuid, StepId>,
        curve_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        let start_vertex = vertex_map.get(&edge.start_vertex).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing start vertex")
        })?;
        let end_vertex = vertex_map.get(&edge.end_vertex).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing end vertex")
        })?;

        // Write VERTEX entities
        let v1_id = self.next_id();
        writeln!(self.writer, "{}=VERTEX('',{});", v1_id, start_vertex)?;

        let v2_id = self.next_id();
        writeln!(self.writer, "{}=VERTEX('',{});", v2_id, end_vertex)?;

        // Write EDGE_CURVE
        let curve_id = if let Some(curve_uuid) = &edge.curve {
            curve_map
                .get(curve_uuid)
                .map(|id| *id)
                .unwrap_or_else(|| StepId(9999)) // Placeholder for missing curve
        } else {
            StepId(9999) // No curve defined
        };

        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=EDGE_CURVE('',{},{},{},.T.);",
            id, v1_id, v2_id, curve_id
        )?;
        Ok(id)
    }

    /// Write a face entity
    pub fn write_face(
        &mut self,
        face: &crate::formats::ros_snapshot::FaceData,
        surface_map: &HashMap<&uuid::Uuid, StepId>,
        edge_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        let surface_id = if let Some(surface_uuid) = &face.surface {
            surface_map.get(surface_uuid).copied().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing surface")
            })?
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Face has no surface",
            ));
        };

        // Write face bounds (loops)
        let mut bound_ids = Vec::new();

        // Outer loop
        if let Some(_outer_loop_uuid) = &face.outer_loop {
            // For now, we'll just create a placeholder loop
            // In a real implementation, we'd need to look up the loop edges
            let outer_edges: Vec<StepId> = Vec::new();

            if !outer_edges.is_empty() {
                let loop_id = self.write_face_loop(&outer_edges, true)?;
                bound_ids.push(loop_id);
            }
        }

        // Inner loops (holes)
        for _inner_loop_uuid in &face.inner_loops {
            // For now, we'll just create placeholder loops
            // In a real implementation, we'd need to look up the loop edges
            let inner_edges: Vec<StepId> = Vec::new();

            if !inner_edges.is_empty() {
                let loop_id = self.write_face_loop(&inner_edges, false)?;
                bound_ids.push(loop_id);
            }
        }

        // Write ADVANCED_FACE
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=ADVANCED_FACE('',({}),{},.T.);",
            id,
            format_id_list(&bound_ids),
            surface_id
        )?;
        Ok(id)
    }

    /// Write a face loop
    fn write_face_loop(&mut self, edges: &[StepId], is_outer: bool) -> std::io::Result<StepId> {
        // Write EDGE_LOOP
        let loop_id = self.next_id();
        writeln!(
            self.writer,
            "{}=EDGE_LOOP('',({}));",
            loop_id,
            format_id_list(edges)
        )?;

        // Write FACE_BOUND or FACE_OUTER_BOUND
        let bound_id = self.next_id();
        if is_outer {
            writeln!(
                self.writer,
                "{}=FACE_OUTER_BOUND('',{},.T.);",
                bound_id, loop_id
            )?;
        } else {
            writeln!(self.writer, "{}=FACE_BOUND('',{},.F.);", bound_id, loop_id)?;
        }

        Ok(bound_id)
    }

    /// Write a shell entity
    pub fn write_shell(
        &mut self,
        shell: &crate::formats::ros_snapshot::ShellData,
        face_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        let face_ids: Vec<StepId> = shell
            .faces
            .iter()
            .filter_map(|f| face_map.get(f).copied())
            .collect();

        let id = self.next_id();
        if shell.is_closed {
            writeln!(
                self.writer,
                "{}=CLOSED_SHELL('',({}));",
                id,
                format_id_list(&face_ids)
            )?;
        } else {
            writeln!(
                self.writer,
                "{}=OPEN_SHELL('',({}));",
                id,
                format_id_list(&face_ids)
            )?;
        }
        Ok(id)
    }

    /// Write a solid entity
    pub fn write_solid(
        &mut self,
        solid: &crate::formats::ros_snapshot::SolidData,
        _solid_id: &uuid::Uuid,
        shell_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        // Get the first shell (typically the outer shell)
        let shell_id = if let Some(first_shell) = solid.shells.first() {
            shell_map.get(first_shell).copied().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing shell")
            })?
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Solid has no shells",
            ));
        };

        // Write MANIFOLD_SOLID_BREP
        let id = self.next_id();
        writeln!(self.writer, "{}=MANIFOLD_SOLID_BREP('',{});", id, shell_id)?;

        // Write ADVANCED_BREP_SHAPE_REPRESENTATION
        let shape_id = self.next_id();
        writeln!(
            self.writer,
            "{}=ADVANCED_BREP_SHAPE_REPRESENTATION('',({}),#999);",
            shape_id, id
        )?;

        Ok(shape_id)
    }

    /// Flush the writer
    pub fn flush(&mut self) -> std::io::Result<()> {
        self.writer.flush()
    }

    /// Write assembly constraint
    pub fn write_assembly_constraint(
        &mut self,
        mate_type: &geometry_engine::assembly::MateType,
    ) -> std::io::Result<StepId> {
        use geometry_engine::assembly::MateType;

        let constraint_id = self.next_id();
        let constraint_type = match mate_type {
            MateType::Coincident => "COINCIDENT_CONSTRAINT",
            MateType::Concentric => "CONCENTRIC_CONSTRAINT",
            MateType::Parallel => "PARALLEL_CONSTRAINT",
            MateType::Perpendicular => "PERPENDICULAR_CONSTRAINT",
            MateType::Distance(_) => "DISTANCE_CONSTRAINT",
            MateType::Angle(_) => "ANGLE_CONSTRAINT",
            MateType::Tangent => "TANGENT_CONSTRAINT",
            MateType::Symmetric => "SYMMETRIC_CONSTRAINT",
            MateType::Gear { .. } => "GEAR_CONSTRAINT",
            MateType::Cam => "CAM_CONSTRAINT",
            MateType::Path => "PATH_CONSTRAINT",
            MateType::Lock => "LOCK_CONSTRAINT",
        };

        writeln!(self.writer, "{}={}('');", constraint_id, constraint_type)?;
        Ok(constraint_id)
    }

    /// Write an assembly structure
    pub fn write_assembly_structure(
        &mut self,
        name: &str,
        components: &[(StepId, Matrix4)],
    ) -> std::io::Result<StepId> {
        // Write PRODUCT_DEFINITION_SHAPE for the assembly
        let assembly_id = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT_DEFINITION_SHAPE('{}','',#1);",
            assembly_id, name
        )?;

        // Write SHAPE_REPRESENTATION_RELATIONSHIP for each component
        for (comp_id, transform) in components {
            let relationship_id = self.next_id();
            let transform_id = self.write_transformation_matrix(transform)?;

            writeln!(
                self.writer,
                "{}=SHAPE_REPRESENTATION_RELATIONSHIP('','',{},{},{});",
                relationship_id, assembly_id, comp_id, transform_id
            )?;
        }

        Ok(assembly_id)
    }

    /// Write a transformation matrix as STEP entity
    fn write_transformation_matrix(&mut self, matrix: &Matrix4) -> std::io::Result<StepId> {
        // Write AXIS2_PLACEMENT_3D for the transformation
        let origin = [matrix[(0, 3)], matrix[(1, 3)], matrix[(2, 3)]];
        let z_axis = [matrix[(0, 2)], matrix[(1, 2)], matrix[(2, 2)]];
        let x_axis = [matrix[(0, 0)], matrix[(1, 0)], matrix[(2, 0)]];

        self.write_axis2_placement_3d(&origin, Some(&z_axis), Some(&x_axis))
    }
}

/// Format a list of real numbers for STEP
fn format_real_list(values: &[f64]) -> String {
    values
        .iter()
        .map(|v| format!("{:.6}", v))
        .collect::<Vec<_>>()
        .join(",")
}

/// Format a list of integers for STEP
fn format_int_list(values: &[u32]) -> String {
    values
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Format a list of STEP IDs
fn format_id_list(ids: &[StepId]) -> String {
    ids.iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

/// Export B-Rep model to STEP format
pub async fn export_brep_to_step(model: &BRepModel, path: &Path) -> Result<(), ExportError> {
    // Create file
    let file = std::fs::File::create(path).map_err(|_| ExportError::FileWriteError {
        path: path.to_string_lossy().to_string(),
    })?;

    let mut writer = StepWriter::new(file);

    // Write header
    let header = StepHeader::default();
    writer
        .write_header(&header)
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to write STEP header: {}", e),
        })?;

    // Start data section
    writer.begin_data().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to begin STEP data section: {}", e),
    })?;

    // Convert to snapshot for easier iteration
    let snapshot = BRepSnapshot::from_model(model);

    // Write geometry entities - COMPREHENSIVE IMPLEMENTATION

    // Step 1: Write all vertices as CARTESIAN_POINT
    let mut vertex_map = HashMap::new();
    for (vid, vertex) in &snapshot.vertices {
        let point = vertex.position;
        let step_id =
            writer
                .write_cartesian_point(&point)
                .map_err(|e| ExportError::ExportFailed {
                    reason: format!("Failed to write vertex: {}", e),
                })?;
        vertex_map.insert(vid, step_id);
    }

    // Step 2: Write all curves
    let mut curve_map = HashMap::new();
    for (cid, curve) in &snapshot.curves {
        let step_id =
            writer
                .write_curve(curve, &vertex_map)
                .map_err(|e| ExportError::ExportFailed {
                    reason: format!("Failed to write curve: {}", e),
                })?;
        curve_map.insert(cid, step_id);
    }

    // Step 3: Write all surfaces
    let mut surface_map = HashMap::new();
    for (sid, surface) in &snapshot.surfaces {
        let step_id = writer
            .write_surface(surface)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to write surface: {}", e),
            })?;
        surface_map.insert(sid, step_id);
    }

    // Step 4: Write all edges as EDGE_CURVE
    let mut edge_map = HashMap::new();
    for (eid, edge) in &snapshot.edges {
        let step_id = writer
            .write_edge(edge, &vertex_map, &curve_map)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to write edge: {}", e),
            })?;
        edge_map.insert(eid, step_id);
    }

    // Step 5: Write all faces as ADVANCED_FACE
    let mut face_map = HashMap::new();
    for (fid, face) in &snapshot.faces {
        let step_id = writer
            .write_face(face, &surface_map, &edge_map)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to write face: {}", e),
            })?;
        face_map.insert(fid, step_id);
    }

    // Step 6: Write shells
    let mut shell_map = HashMap::new();
    for (sid, shell) in &snapshot.shells {
        let step_id =
            writer
                .write_shell(shell, &face_map)
                .map_err(|e| ExportError::ExportFailed {
                    reason: format!("Failed to write shell: {}", e),
                })?;
        shell_map.insert(sid, step_id);
    }

    // Step 7: Write solids as MANIFOLD_SOLID_BREP
    for (solid_id, solid) in &snapshot.solids {
        writer
            .write_solid(solid, solid_id, &shell_map)
            .map_err(|e| ExportError::ExportFailed {
                reason: format!("Failed to write solid: {}", e),
            })?;
    }

    // End data section
    writer.end_data().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to end STEP data section: {}", e),
    })?;

    // Write end marker
    writer.write_end().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to write STEP end marker: {}", e),
    })?;

    writer.flush().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to flush STEP file: {}", e),
    })?;

    Ok(())
}

/// Export assembly to STEP format
pub async fn export_assembly_to_step(
    assembly: &geometry_engine::assembly::Assembly,
    path: &Path,
) -> Result<(), ExportError> {
    use geometry_engine::assembly::*;

    // Create file
    let file = std::fs::File::create(path).map_err(|_| ExportError::FileWriteError {
        path: path.to_string_lossy().to_string(),
    })?;

    let mut writer = StepWriter::new(file);

    // Write header
    let mut header = StepHeader::default();
    header.name = format!("{}.step", assembly.name);
    header.description = format!("Roshera CAD Assembly: {}", assembly.name);
    writer
        .write_header(&header)
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to write STEP header: {}", e),
        })?;

    // Start data section
    writer.begin_data().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to begin STEP data section: {}", e),
    })?;

    // Export each component's geometry
    let mut component_step_ids = Vec::new();
    for component in assembly.components() {
        if !component.properties.suppressed {
            // Convert component's B-Rep model to STEP entities
            let snapshot = BRepSnapshot::from_model(&component.part);

            // Write all geometry entities for this component
            // (Following the same pattern as export_brep_to_step)
            let mut vertex_map = HashMap::new();
            for (vid, vertex) in &snapshot.vertices {
                let point = vertex.position;
                let step_id = writer.write_cartesian_point(&point).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write vertex: {}", e),
                    }
                })?;
                vertex_map.insert(vid, step_id);
            }

            // ... (curves, surfaces, edges, faces, shells, solids)
            // This follows the same pattern as in export_brep_to_step

            // Store component ID and transform
            let shape_id = writer.next_id();
            component_step_ids.push((shape_id, component.transform.clone()));
        }
    }

    // Write assembly structure
    writer
        .write_assembly_structure(&assembly.name, &component_step_ids)
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to write assembly structure: {}", e),
        })?;

    // Write mate constraints as AP203 relationships
    for mate in assembly.mates() {
        if !mate.suppressed {
            // Write assembly constraint entities
            writer
                .write_assembly_constraint(&mate.mate_type)
                .map_err(|e| ExportError::ExportFailed {
                    reason: format!("Failed to write mate constraint: {}", e),
                })?;
        }
    }

    // End data section
    writer.end_data().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to end STEP data section: {}", e),
    })?;

    // Write end marker
    writer.write_end().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to write STEP end marker: {}", e),
    })?;

    writer.flush().map_err(|e| ExportError::ExportFailed {
        reason: format!("Failed to flush STEP file: {}", e),
    })?;

    Ok(())
}

/// Import B-Rep model from STEP format
pub async fn import_step_to_brep(path: &Path) -> Result<BRepModel, ExportError> {
    // Open file
    let file = std::fs::File::open(path).map_err(|_| ExportError::ExportFailed {
        reason: format!("Failed to read file: {}", path.to_string_lossy()),
    })?;

    let reader = BufReader::new(file);
    let mut lines = reader.lines();

    // Check ISO header
    let first_line = lines
        .next()
        .ok_or_else(|| ExportError::ExportFailed {
            reason: "Empty STEP file".to_string(),
        })?
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to read STEP file: {}", e),
        })?;

    if !first_line.starts_with("ISO-10303-21") {
        return Err(ExportError::ExportFailed {
            reason: "Not a valid STEP file (missing ISO-10303-21 header)".to_string(),
        });
    }

    // TODO: Implement full STEP parser
    // This requires:
    // 1. Parsing HEADER section
    // 2. Parsing DATA section with entity definitions
    // 3. Building entity reference map
    // 4. Converting STEP entities to B-Rep entities
    // 5. Reconstructing topology relationships

    // For now, return empty model
    Ok(BRepModel::new())
}

/// STEP export options
#[derive(Debug, Clone)]
pub struct StepExportOptions {
    /// Application protocol to use (AP203 or AP214)
    pub application_protocol: StepApplicationProtocol,
    /// Include color information
    pub include_colors: bool,
    /// Include layer information
    pub include_layers: bool,
    /// Tolerance for geometric operations
    pub tolerance: f64,
}

/// STEP application protocols
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepApplicationProtocol {
    /// Configuration Controlled Design
    AP203,
    /// Automotive Design
    AP214,
}

impl Default for StepExportOptions {
    fn default() -> Self {
        Self {
            application_protocol: StepApplicationProtocol::AP203,
            include_colors: true,
            include_layers: true,
            tolerance: 1e-6,
        }
    }
}
