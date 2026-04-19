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
                // Write basis CIRCLE entity first
                let basis_circle_id = self.write_circle(
                    &[center[0], center[1], center[2]],
                    &[normal[0], normal[1], normal[2]],
                    *radius,
                )?;

                // Write trim parameter points
                let trim1_id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=PARAMETER_VALUE({});",
                    trim1_id, start_angle
                )?;
                let trim2_id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=PARAMETER_VALUE({});",
                    trim2_id, end_angle
                )?;

                // Write TRIMMED_CURVE referencing the basis circle
                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=TRIMMED_CURVE('',{},({}),({}),{},.PARAMETER.);",
                    id, basis_circle_id, trim1_id, trim2_id,
                    if end_angle > start_angle { ".T." } else { ".F." }
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

    /// Write a LINE entity using already-written vertex STEP IDs as endpoints
    fn write_line_from_vertices(
        &mut self,
        start_vertex_id: StepId,
        end_vertex_id: StepId,
    ) -> std::io::Result<StepId> {
        // Create a direction placeholder (unit X — the actual geometry is defined by vertices)
        let dir_id = self.write_direction(&[1.0, 0.0, 0.0])?;
        let vec_id = self.next_id();
        writeln!(self.writer, "{}=VECTOR('',{},1.0);", vec_id, dir_id)?;

        let id = self.next_id();
        writeln!(self.writer, "{}=LINE('',{},{});", id, start_vertex_id, vec_id)?;
        Ok(id)
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

        // Write EDGE_CURVE — if no curve exists, synthesize a straight line
        let curve_id = if let Some(curve_uuid) = &edge.curve {
            if let Some(&id) = curve_map.get(curve_uuid) {
                id
            } else {
                // Curve UUID exists but wasn't written — create a line from start to end vertex
                self.write_line_from_vertices(*start_vertex, *end_vertex)?
            }
        } else {
            // No curve defined — create a straight line from start to end vertex
            self.write_line_from_vertices(*start_vertex, *end_vertex)?
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
        loop_map: &HashMap<&uuid::Uuid, &crate::formats::ros_snapshot::LoopData>,
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

        // Outer loop — look up edges from loop data
        if let Some(outer_loop_uuid) = &face.outer_loop {
            if let Some(loop_data) = loop_map.get(outer_loop_uuid) {
                let outer_edges: Vec<StepId> = loop_data
                    .edges
                    .iter()
                    .filter_map(|edge_uuid| edge_map.get(edge_uuid).copied())
                    .collect();

                if !outer_edges.is_empty() {
                    let loop_id = self.write_face_loop(&outer_edges, true)?;
                    bound_ids.push(loop_id);
                }
            }
        }

        // Inner loops (holes) — look up edges from loop data
        for inner_loop_uuid in &face.inner_loops {
            if let Some(loop_data) = loop_map.get(inner_loop_uuid) {
                let inner_edges: Vec<StepId> = loop_data
                    .edges
                    .iter()
                    .filter_map(|edge_uuid| edge_map.get(edge_uuid).copied())
                    .collect();

                if !inner_edges.is_empty() {
                    let loop_id = self.write_face_loop(&inner_edges, false)?;
                    bound_ids.push(loop_id);
                }
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

        // Write geometric context for the shape representation
        let context_id = self.write_geometric_context()?;

        // Write ADVANCED_BREP_SHAPE_REPRESENTATION
        let shape_id = self.next_id();
        writeln!(
            self.writer,
            "{}=ADVANCED_BREP_SHAPE_REPRESENTATION('',({},{}),{});",
            shape_id, id, context_id, context_id
        )?;

        Ok(shape_id)
    }

    /// Write a geometric representation context (required by ADVANCED_BREP_SHAPE_REPRESENTATION)
    fn write_geometric_context(&mut self) -> std::io::Result<StepId> {
        // Length unit: millimeters
        let mm_id = self.next_id();
        writeln!(self.writer, "{}=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );", mm_id)?;

        // Angle unit: radians
        let rad_id = self.next_id();
        writeln!(self.writer, "{}=( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );", rad_id)?;

        // Solid angle unit: steradians
        let sr_id = self.next_id();
        writeln!(self.writer, "{}=( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );", sr_id)?;

        // Uncertainty measure
        let uncertainty_id = self.next_id();
        writeln!(
            self.writer,
            "{}=UNCERTAINTY_MEASURE_WITH_UNIT(LENGTH_MEASURE(1.E-07),{},'distance_accuracy_value','Maximum model space distance');",
            uncertainty_id, mm_id
        )?;

        // Geometric representation context
        let ctx_id = self.next_id();
        writeln!(
            self.writer,
            "{}=( GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNCERTAINTY_ASSIGNED_CONTEXT(({uncertainty})) GLOBAL_UNIT_ASSIGNED_CONTEXT(({mm},{rad},{sr})) REPRESENTATION_CONTEXT('Context3D','3D Context with 1e-7 uncertainty') );",
            ctx_id,
            uncertainty = uncertainty_id,
            mm = mm_id,
            rad = rad_id,
            sr = sr_id
        )?;

        Ok(ctx_id)
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

    // Build loop lookup map: Uuid -> &LoopData
    let loop_map: HashMap<&Uuid, &crate::formats::ros_snapshot::LoopData> = snapshot
        .loops
        .iter()
        .map(|(id, data)| (id, data))
        .collect();

    // Step 5: Write all faces as ADVANCED_FACE
    let mut face_map = HashMap::new();
    for (fid, face) in &snapshot.faces {
        let step_id = writer
            .write_face(face, &surface_map, &edge_map, &loop_map)
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
            let mut vertex_map = HashMap::new();
            for (vid, vertex) in &snapshot.vertices {
                let step_id = writer.write_cartesian_point(&vertex.position).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write vertex: {}", e),
                    }
                })?;
                vertex_map.insert(vid, step_id);
            }

            let mut curve_map = HashMap::new();
            for (cid, curve) in &snapshot.curves {
                let step_id = writer.write_curve(curve, &vertex_map).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write curve: {}", e),
                    }
                })?;
                curve_map.insert(cid, step_id);
            }

            let mut surface_map = HashMap::new();
            for (sid, surface) in &snapshot.surfaces {
                let step_id = writer.write_surface(surface).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write surface: {}", e),
                    }
                })?;
                surface_map.insert(sid, step_id);
            }

            let mut edge_map = HashMap::new();
            for (eid, edge) in &snapshot.edges {
                let step_id = writer.write_edge(edge, &vertex_map, &curve_map).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write edge: {}", e),
                    }
                })?;
                edge_map.insert(eid, step_id);
            }

            let loop_map: HashMap<&Uuid, &crate::formats::ros_snapshot::LoopData> = snapshot
                .loops
                .iter()
                .map(|(id, data)| (id, data))
                .collect();

            let mut face_map = HashMap::new();
            for (fid, face) in &snapshot.faces {
                let step_id = writer.write_face(face, &surface_map, &edge_map, &loop_map).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write face: {}", e),
                    }
                })?;
                face_map.insert(fid, step_id);
            }

            let mut shell_map = HashMap::new();
            for (sid, shell) in &snapshot.shells {
                let step_id = writer.write_shell(shell, &face_map).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write shell: {}", e),
                    }
                })?;
                shell_map.insert(sid, step_id);
            }

            // Write solids and capture the last shape ID for this component
            let mut last_shape_id = writer.next_id(); // fallback
            for (solid_id, solid) in &snapshot.solids {
                last_shape_id = writer.write_solid(solid, solid_id, &shell_map).map_err(|e| {
                    ExportError::ExportFailed {
                        reason: format!("Failed to write solid: {}", e),
                    }
                })?;
            }

            component_step_ids.push((last_shape_id, component.transform.clone()));
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

    // Parse all lines into a single buffer, stripping comments
    let mut data_section = String::new();
    let mut in_data = false;
    for line_result in lines {
        let line = line_result.map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to read STEP file: {}", e),
        })?;
        let trimmed = line.trim();
        if trimmed == "DATA;" {
            in_data = true;
            continue;
        }
        if trimmed == "ENDSEC;" && in_data {
            break;
        }
        if in_data {
            data_section.push_str(trimmed);
            data_section.push(' ');
        }
    }

    // Parse entities: #N=TYPE(...);
    let entities = parse_step_entities(&data_section)?;

    // Build B-Rep model from parsed entities
    let model = reconstruct_brep_from_step(&entities)?;

    Ok(model)
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

// ── STEP Import Parser ──────────────────────────────────────────────────────

/// A parsed STEP entity with its type name and raw argument string
#[derive(Debug, Clone)]
struct StepEntity {
    id: u32,
    type_name: String,
    args: String,
}

/// Parse all `#N=TYPE(...)` entities from the DATA section text
fn parse_step_entities(data: &str) -> Result<HashMap<u32, StepEntity>, ExportError> {
    let mut entities = HashMap::new();
    // Regex-free parser: split on `;` then parse each statement
    for statement in data.split(';') {
        let stmt = statement.trim();
        if stmt.is_empty() {
            continue;
        }
        // Find #N=TYPE(...)
        let Some(hash_pos) = stmt.find('#') else {
            continue;
        };
        let after_hash = &stmt[hash_pos + 1..];
        let Some(eq_pos) = after_hash.find('=') else {
            continue;
        };
        let id_str = after_hash[..eq_pos].trim();
        let Ok(id) = id_str.parse::<u32>() else {
            continue;
        };
        let rhs = after_hash[eq_pos + 1..].trim();
        // Find type name (everything before first '(')
        let Some(paren_pos) = rhs.find('(') else {
            continue;
        };
        let type_name = rhs[..paren_pos].trim().to_uppercase();
        // Extract args between outermost parens
        let args_start = paren_pos + 1;
        // Find matching closing paren (handle nesting)
        let mut depth = 1;
        let mut args_end = args_start;
        for (i, ch) in rhs[args_start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        args_end = args_start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let args = rhs[args_start..args_end].to_string();
        entities.insert(id, StepEntity { id, type_name, args });
    }
    Ok(entities)
}

/// Parse a comma-separated list of f64 values from a STEP argument like "1.0,2.0,3.0"
fn parse_real_list(s: &str) -> Vec<f64> {
    s.split(',')
        .filter_map(|v| v.trim().parse::<f64>().ok())
        .collect()
}

/// Parse a STEP entity reference like "#123" into a u32 ID
fn parse_ref(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.starts_with('#') {
        s[1..].parse::<u32>().ok()
    } else {
        None
    }
}

/// Parse a comma-separated list of STEP references like "#1,#2,#3"
fn parse_ref_list(s: &str) -> Vec<u32> {
    s.split(',').filter_map(|v| parse_ref(v)).collect()
}

/// Split top-level comma-separated args (respecting nested parens)
fn split_step_args(args: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for ch in args.chars() {
        match ch {
            '(' => {
                depth += 1;
                current.push(ch);
            }
            ')' => {
                depth -= 1;
                current.push(ch);
            }
            ',' if depth == 0 => {
                result.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    let last = current.trim().to_string();
    if !last.is_empty() {
        result.push(last);
    }
    result
}

/// Extract coordinates from a CARTESIAN_POINT entity
fn extract_point(entities: &HashMap<u32, StepEntity>, id: u32) -> Option<[f64; 3]> {
    let entity = entities.get(&id)?;
    if entity.type_name != "CARTESIAN_POINT" {
        return None;
    }
    let args = split_step_args(&entity.args);
    // args[0] = name string, args[1] = (x,y,z)
    if args.len() < 2 {
        return None;
    }
    let coords_str = args[1].trim();
    let coords_inner = coords_str
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(coords_str);
    let vals = parse_real_list(coords_inner);
    if vals.len() >= 3 {
        Some([vals[0], vals[1], vals[2]])
    } else if vals.len() == 2 {
        Some([vals[0], vals[1], 0.0])
    } else {
        None
    }
}

/// Extract direction from a DIRECTION entity
fn extract_direction(entities: &HashMap<u32, StepEntity>, id: u32) -> Option<[f64; 3]> {
    let entity = entities.get(&id)?;
    if entity.type_name != "DIRECTION" {
        return None;
    }
    let args = split_step_args(&entity.args);
    if args.len() < 2 {
        return None;
    }
    let coords_str = args[1].trim();
    let coords_inner = coords_str
        .strip_prefix('(')
        .and_then(|s| s.strip_suffix(')'))
        .unwrap_or(coords_str);
    let vals = parse_real_list(coords_inner);
    if vals.len() >= 3 {
        Some([vals[0], vals[1], vals[2]])
    } else {
        None
    }
}

/// Extract location and axis from AXIS2_PLACEMENT_3D
fn extract_axis2_placement(
    entities: &HashMap<u32, StepEntity>,
    id: u32,
) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
    let entity = entities.get(&id)?;
    if entity.type_name != "AXIS2_PLACEMENT_3D" {
        return None;
    }
    let args = split_step_args(&entity.args);
    // args: name, location_ref, axis_ref, ref_direction_ref
    if args.len() < 2 {
        return None;
    }
    let location = parse_ref(&args[1]).and_then(|r| extract_point(entities, r))?;
    let axis = if args.len() > 2 {
        parse_ref(&args[2])
            .and_then(|r| extract_direction(entities, r))
            .unwrap_or([0.0, 0.0, 1.0])
    } else {
        [0.0, 0.0, 1.0]
    };
    let ref_dir = if args.len() > 3 {
        parse_ref(&args[3])
            .and_then(|r| extract_direction(entities, r))
            .unwrap_or([1.0, 0.0, 0.0])
    } else {
        [1.0, 0.0, 0.0]
    };
    Some((location, axis, ref_dir))
}

/// Reconstruct a BRepModel from parsed STEP entities
fn reconstruct_brep_from_step(
    entities: &HashMap<u32, StepEntity>,
) -> Result<BRepModel, ExportError> {
    use geometry_engine::math::{Point3, Tolerance, Vector3};
    use geometry_engine::primitives::{
        curve::ParameterRange,
        edge::{Edge, EdgeOrientation},
        face::FaceOrientation,
        r#loop::{Loop, LoopType},
        shell::{Shell, ShellType},
        solid::Solid,
    };

    let mut model = BRepModel::new();
    let tolerance = Tolerance::default();

    // Type aliases for readability
    type VertexId = geometry_engine::primitives::vertex::VertexId;
    type CurveId = geometry_engine::primitives::curve::CurveId;
    type SurfaceId = geometry_engine::primitives::surface::SurfaceId;
    type EdgeId = geometry_engine::primitives::edge::EdgeId;
    type LoopId = geometry_engine::primitives::r#loop::LoopId;
    type FaceId = geometry_engine::primitives::face::FaceId;
    type ShellId = geometry_engine::primitives::shell::ShellId;

    // Map from STEP entity ID -> internal store ID for each entity type
    let mut vertex_id_map: HashMap<u32, VertexId> = HashMap::new();
    let mut curve_id_map: HashMap<u32, CurveId> = HashMap::new();
    let mut surface_id_map: HashMap<u32, SurfaceId> = HashMap::new();
    let mut edge_id_map: HashMap<u32, EdgeId> = HashMap::new();

    // Pass 1: Import CARTESIAN_POINTs as vertices
    for (&step_id, entity) in entities {
        if entity.type_name == "CARTESIAN_POINT" {
            if let Some(coords) = extract_point(entities, step_id) {
                let vid = model.vertices.add_or_find(
                    coords[0],
                    coords[1],
                    coords[2],
                    tolerance.distance(),
                );
                vertex_id_map.insert(step_id, vid);
            }
        }
    }

    // Pass 2: Import curves (LINE, CIRCLE, B_SPLINE_CURVE_WITH_KNOTS)
    for (&step_id, entity) in entities {
        match entity.type_name.as_str() {
            "LINE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 3 {
                    if let Some(start_ref) = parse_ref(&args[1]) {
                        if let Some(start_pt) = extract_point(entities, start_ref) {
                            // Create a line curve
                            let p1 = Point3::new(start_pt[0], start_pt[1], start_pt[2]);
                            let line = geometry_engine::primitives::curve::Line::new(
                                p1,
                                // Direction from VECTOR entity
                                Vector3::new(1.0, 0.0, 0.0), // default, refined below
                            );
                            // Try to extract actual direction from the vector ref
                            if let Some(vec_ref) = parse_ref(&args[2]) {
                                if let Some(vec_entity) = entities.get(&vec_ref) {
                                    if vec_entity.type_name == "VECTOR" {
                                        let vec_args = split_step_args(&vec_entity.args);
                                        if vec_args.len() >= 2 {
                                            if let Some(dir_ref) = parse_ref(&vec_args[1]) {
                                                if let Some(dir) =
                                                    extract_direction(entities, dir_ref)
                                                {
                                                    let line =
                                                        geometry_engine::primitives::curve::Line::new(
                                                            p1,
                                                            Vector3::new(dir[0], dir[1], dir[2]),
                                                        );
                                                    let cid =
                                                        model.curves.add(Box::new(line));
                                                    curve_id_map.insert(step_id, cid);
                                                    continue;
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            let cid = model.curves.add(Box::new(line));
                            curve_id_map.insert(step_id, cid);
                        }
                    }
                }
            }
            "CIRCLE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 3 {
                    if let Some(axis_ref) = parse_ref(&args[1]) {
                        if let Some((center, normal, _ref_dir)) =
                            extract_axis2_placement(entities, axis_ref)
                        {
                            if let Ok(radius) = args[2].trim().parse::<f64>() {
                                let arc = geometry_engine::primitives::curve::Arc::new(
                                    Point3::new(center[0], center[1], center[2]),
                                    Vector3::new(normal[0], normal[1], normal[2]),
                                    radius,
                                    0.0,
                                    std::f64::consts::TAU,
                                );
                                if let Ok(arc) = arc {
                                    let cid = model.curves.add(Box::new(arc));
                                    curve_id_map.insert(step_id, cid);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Pass 3: Import surfaces (PLANE, CYLINDRICAL_SURFACE, SPHERICAL_SURFACE, etc.)
    for (&step_id, entity) in entities {
        match entity.type_name.as_str() {
            "PLANE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 2 {
                    if let Some(axis_ref) = parse_ref(&args[1]) {
                        if let Some((origin, normal, ref_dir)) =
                            extract_axis2_placement(entities, axis_ref)
                        {
                            let plane = geometry_engine::primitives::surface::Plane::new(
                                Point3::new(origin[0], origin[1], origin[2]),
                                Vector3::new(normal[0], normal[1], normal[2]),
                                Vector3::new(ref_dir[0], ref_dir[1], ref_dir[2]),
                            );
                            if let Ok(plane) = plane {
                                let sid = model.surfaces.add(Box::new(plane));
                                surface_id_map.insert(step_id, sid);
                            }
                        }
                    }
                }
            }
            "CYLINDRICAL_SURFACE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 3 {
                    if let Some(axis_ref) = parse_ref(&args[1]) {
                        if let Some((origin, axis, _)) =
                            extract_axis2_placement(entities, axis_ref)
                        {
                            if let Ok(radius) = args[2].trim().parse::<f64>() {
                                let cyl = geometry_engine::primitives::surface::Cylinder::new(
                                    Point3::new(origin[0], origin[1], origin[2]),
                                    Vector3::new(axis[0], axis[1], axis[2]),
                                    radius,
                                );
                                if let Ok(cyl) = cyl {
                                    let sid = model.surfaces.add(Box::new(cyl));
                                    surface_id_map.insert(step_id, sid);
                                }
                            }
                        }
                    }
                }
            }
            "SPHERICAL_SURFACE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 3 {
                    if let Some(axis_ref) = parse_ref(&args[1]) {
                        if let Some((center, _, _)) =
                            extract_axis2_placement(entities, axis_ref)
                        {
                            if let Ok(radius) = args[2].trim().parse::<f64>() {
                                let sphere = geometry_engine::primitives::surface::Sphere::new(
                                    Point3::new(center[0], center[1], center[2]),
                                    radius,
                                );
                                if let Ok(sphere) = sphere {
                                    let sid = model.surfaces.add(Box::new(sphere));
                                    surface_id_map.insert(step_id, sid);
                                }
                            }
                        }
                    }
                }
            }
            "CONICAL_SURFACE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 4 {
                    if let Some(axis_ref) = parse_ref(&args[1]) {
                        if let Some((apex, axis, _)) =
                            extract_axis2_placement(entities, axis_ref)
                        {
                            if let (Ok(_radius), Ok(half_angle_deg)) = (
                                args[2].trim().parse::<f64>(),
                                args[3].trim().parse::<f64>(),
                            ) {
                                let cone = geometry_engine::primitives::surface::Cone::new(
                                    Point3::new(apex[0], apex[1], apex[2]),
                                    Vector3::new(axis[0], axis[1], axis[2]),
                                    half_angle_deg.to_radians(),
                                );
                                if let Ok(cone) = cone {
                                    let sid = model.surfaces.add(Box::new(cone));
                                    surface_id_map.insert(step_id, sid);
                                }
                            }
                        }
                    }
                }
            }
            "TOROIDAL_SURFACE" => {
                let args = split_step_args(&entity.args);
                if args.len() >= 4 {
                    if let Some(axis_ref) = parse_ref(&args[1]) {
                        if let Some((center, axis, _)) =
                            extract_axis2_placement(entities, axis_ref)
                        {
                            if let (Ok(major_r), Ok(minor_r)) = (
                                args[2].trim().parse::<f64>(),
                                args[3].trim().parse::<f64>(),
                            ) {
                                let torus = geometry_engine::primitives::surface::Torus::new(
                                    Point3::new(center[0], center[1], center[2]),
                                    Vector3::new(axis[0], axis[1], axis[2]),
                                    major_r,
                                    minor_r,
                                );
                                if let Ok(torus) = torus {
                                    let sid = model.surfaces.add(Box::new(torus));
                                    surface_id_map.insert(step_id, sid);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // Pass 4: Import EDGE_CURVE entities
    for (&step_id, entity) in entities {
        if entity.type_name == "EDGE_CURVE" {
            let args = split_step_args(&entity.args);
            // args: name, start_vertex_ref, end_vertex_ref, curve_ref, same_sense
            if args.len() >= 4 {
                // Resolve vertex references (VERTEX_POINT -> CARTESIAN_POINT)
                let start_vid = parse_ref(&args[1])
                    .and_then(|vr| resolve_vertex_point(entities, vr))
                    .and_then(|pt_id| vertex_id_map.get(&pt_id).copied());
                let end_vid = parse_ref(&args[2])
                    .and_then(|vr| resolve_vertex_point(entities, vr))
                    .and_then(|pt_id| vertex_id_map.get(&pt_id).copied());
                let curve_cid = parse_ref(&args[3]).and_then(|cr| curve_id_map.get(&cr).copied());

                if let (Some(sv), Some(ev)) = (start_vid, end_vid) {
                    // Use a default curve if not found
                    let cid = curve_cid.unwrap_or_else(|| {
                        // Create a placeholder line curve
                        let line = geometry_engine::primitives::curve::Line::new(
                            Point3::new(0.0, 0.0, 0.0),
                            Vector3::new(1.0, 0.0, 0.0),
                        );
                        model.curves.add(Box::new(line))
                    });

                    let edge = Edge::new(
                        0,
                        sv,
                        ev,
                        cid,
                        EdgeOrientation::Forward,
                        ParameterRange::new(0.0, 1.0),
                    );
                    let eid = model.edges.add(edge);
                    edge_id_map.insert(step_id, eid);
                }
            }
        }
    }

    // Pass 5: Import EDGE_LOOP -> Loop, FACE_OUTER_BOUND/FACE_BOUND -> Face, CLOSED_SHELL/OPEN_SHELL -> Shell
    let mut loop_id_map: HashMap<u32, LoopId> = HashMap::new();
    let mut face_bound_loop_map: HashMap<u32, (LoopId, bool)> = HashMap::new();
    let mut face_id_map: HashMap<u32, FaceId> = HashMap::new();
    let mut shell_id_map: HashMap<u32, ShellId> = HashMap::new();

    // 5a: EDGE_LOOPs
    for (&step_id, entity) in entities {
        if entity.type_name == "EDGE_LOOP" {
            let args = split_step_args(&entity.args);
            if args.len() >= 2 {
                let refs_str = args[1].trim();
                let refs_inner = refs_str
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(refs_str);
                let edge_refs = parse_ref_list(refs_inner);

                let mut lp = Loop::new(0, LoopType::Outer);
                for er in &edge_refs {
                    // Edge refs might be ORIENTED_EDGE entities
                    if let Some(eid) = resolve_oriented_edge(entities, *er, &edge_id_map) {
                        lp.add_edge(eid, true);
                    }
                }
                let lid = model.loops.add(lp);
                loop_id_map.insert(step_id, lid);
            }
        }
    }

    // 5b: FACE_OUTER_BOUND and FACE_BOUND
    for (&step_id, entity) in entities {
        if entity.type_name == "FACE_OUTER_BOUND" || entity.type_name == "FACE_BOUND" {
            let args = split_step_args(&entity.args);
            if args.len() >= 2 {
                let is_outer = entity.type_name == "FACE_OUTER_BOUND";
                if let Some(loop_ref) = parse_ref(&args[1]) {
                    if let Some(&lid) = loop_id_map.get(&loop_ref) {
                        face_bound_loop_map.insert(step_id, (lid, is_outer));
                    }
                }
            }
        }
    }

    // 5c: ADVANCED_FACE
    for (&step_id, entity) in entities {
        if entity.type_name == "ADVANCED_FACE" {
            let args = split_step_args(&entity.args);
            // args: name, (bound_refs), surface_ref, same_sense
            if args.len() >= 3 {
                let bounds_str = args[1].trim();
                let bounds_inner = bounds_str
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(bounds_str);
                let bound_refs = parse_ref_list(bounds_inner);

                let surface_ref = parse_ref(&args[2]);
                let sid = surface_ref.and_then(|sr| surface_id_map.get(&sr).copied());

                // Find outer loop from bounds
                let mut outer_loop_id = None;
                for &br in &bound_refs {
                    if let Some(&(lid, is_outer)) = face_bound_loop_map.get(&br) {
                        if is_outer {
                            outer_loop_id = Some(lid);
                            break;
                        }
                    }
                }
                // If no explicit outer, take the first bound
                if outer_loop_id.is_none() {
                    for &br in &bound_refs {
                        if let Some(&(lid, _)) = face_bound_loop_map.get(&br) {
                            outer_loop_id = Some(lid);
                            break;
                        }
                    }
                }

                if let (Some(surface_id), Some(loop_id)) = (sid, outer_loop_id) {
                    let face = geometry_engine::primitives::face::Face::new(
                        0,
                        surface_id,
                        loop_id,
                        FaceOrientation::Forward,
                    );
                    let fid = model.faces.add(face);
                    face_id_map.insert(step_id, fid);
                }
            }
        }
    }

    // 5d: CLOSED_SHELL and OPEN_SHELL
    for (&step_id, entity) in entities {
        if entity.type_name == "CLOSED_SHELL" || entity.type_name == "OPEN_SHELL" {
            let args = split_step_args(&entity.args);
            if args.len() >= 2 {
                let faces_str = args[1].trim();
                let faces_inner = faces_str
                    .strip_prefix('(')
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(faces_str);
                let face_refs = parse_ref_list(faces_inner);

                let shell_type = if entity.type_name == "CLOSED_SHELL" {
                    ShellType::Closed
                } else {
                    ShellType::Open
                };
                let mut shell = Shell::new(0, shell_type);
                for &fr in &face_refs {
                    if let Some(&fid) = face_id_map.get(&fr) {
                        shell.add_face(fid);
                    }
                }
                let shell_id = model.shells.add(shell);
                shell_id_map.insert(step_id, shell_id);
            }
        }
    }

    // Pass 6: Import MANIFOLD_SOLID_BREP
    for (_step_id, entity) in entities {
        if entity.type_name == "MANIFOLD_SOLID_BREP" {
            let args = split_step_args(&entity.args);
            if args.len() >= 2 {
                if let Some(shell_ref) = parse_ref(&args[1]) {
                    if let Some(&shell_id) = shell_id_map.get(&shell_ref) {
                        let solid = Solid::new(0, shell_id);
                        model.solids.add(solid);
                    }
                }
            }
        }
    }

    Ok(model)
}

/// Resolve a VERTEX or VERTEX_POINT entity to its CARTESIAN_POINT ID
fn resolve_vertex_point(entities: &HashMap<u32, StepEntity>, vertex_ref: u32) -> Option<u32> {
    let entity = entities.get(&vertex_ref)?;
    match entity.type_name.as_str() {
        "VERTEX_POINT" | "VERTEX" => {
            let args = split_step_args(&entity.args);
            // args: name, point_ref
            if args.len() >= 2 {
                parse_ref(&args[1])
            } else {
                None
            }
        }
        "CARTESIAN_POINT" => Some(vertex_ref),
        _ => None,
    }
}

/// Resolve an ORIENTED_EDGE to get the underlying edge ID
fn resolve_oriented_edge(
    entities: &HashMap<u32, StepEntity>,
    oriented_edge_ref: u32,
    edge_id_map: &HashMap<u32, geometry_engine::primitives::edge::EdgeId>,
) -> Option<geometry_engine::primitives::edge::EdgeId> {
    let entity = entities.get(&oriented_edge_ref)?;
    match entity.type_name.as_str() {
        "ORIENTED_EDGE" => {
            let args = split_step_args(&entity.args);
            // args: name, *, *, edge_ref, orientation
            if args.len() >= 4 {
                let edge_ref = parse_ref(&args[3])?;
                edge_id_map.get(&edge_ref).copied()
            } else {
                None
            }
        }
        "EDGE_CURVE" => edge_id_map.get(&oriented_edge_ref).copied(),
        _ => None,
    }
}
