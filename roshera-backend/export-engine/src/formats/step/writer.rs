//! STEP file format writer (ISO 10303-21 export).
//!
//! Builds an ASCII exchange structure for a `BRepModel` or `Assembly` and
//! writes it to disk. Defaults to AP242 (Managed Model-Based 3D
//! Engineering, MIM long-form). AP214 (Automotive Design) and AP203
//! (Configuration Controlled Design) remain selectable through
//! [`StepExportOptions::application_protocol`] for round-trip parity
//! with legacy systems, but new Roshera exports always declare AP242.
//!
//! The import path is **not** in this module — see `super::mod` for the
//! parser + dispatch architecture. This file is intentionally
//! export-only so the writer can evolve independently of the importer
//! (IMP5 in `plans/step-import-universal.md`).

use crate::formats::ros_snapshot::BRepSnapshot;
use chrono::{DateTime, Utc};
use geometry_engine::math::matrix4::Matrix4;
use geometry_engine::primitives::topology_builder::BRepModel;
use shared_types::*;
use std::collections::HashMap;
use std::io::{BufWriter, Write};
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
    /// Application protocol declared in the FILE_SCHEMA header.
    protocol: StepApplicationProtocol,
}

impl<W: Write> StepWriter<W> {
    /// Create a new STEP writer targeting the default application
    /// protocol (AP242).
    pub fn new(writer: W) -> Self {
        Self::with_protocol(writer, StepApplicationProtocol::default())
    }

    /// Create a new STEP writer with an explicit application protocol.
    /// The protocol drives the `FILE_SCHEMA` string emitted by
    /// [`Self::write_header`].
    pub fn with_protocol(writer: W, protocol: StepApplicationProtocol) -> Self {
        Self {
            writer: BufWriter::new(writer),
            entity_counter: 1,
            protocol,
        }
    }

    /// The application protocol this writer declares in its header.
    pub fn protocol(&self) -> StepApplicationProtocol {
        self.protocol
    }

    /// Get the next entity ID
    fn next_id(&mut self) -> StepId {
        let id = StepId(self.entity_counter);
        self.entity_counter += 1;
        id
    }

    /// Write the STEP header
    pub fn write_header(&mut self, header: &StepHeader) -> std::io::Result<()> {
        writeln!(self.writer, "ISO-10303-21;")?;
        writeln!(self.writer, "HEADER;")?;

        // FILE_DESCRIPTION — ISO 10303-21 §8 requires the trailing `;`.
        // Its absence made the entire header unparseable (the importer
        // failed on the FILE_NAME record that ran into it).
        writeln!(
            self.writer,
            "FILE_DESCRIPTION(('{}'),'{}');",
            escape_step_string(&header.description),
            escape_step_string(&header.implementation_level)
        )?;

        // FILE_NAME — `author` and `organization` are STRING-list fields
        // in the schema (`(author)`, `(organization)`); emit each as a
        // single-element list. Trailing `;` is mandatory.
        writeln!(
            self.writer,
            "FILE_NAME('{}','{}',('{}'),('{}'),'{}','{}','{}');",
            escape_step_string(&header.name),
            header.time_stamp.format("%Y-%m-%dT%H:%M:%S"),
            escape_step_string(&header.author),
            escape_step_string(&header.organization),
            escape_step_string(&header.preprocessor_version),
            escape_step_string(&header.originating_system),
            escape_step_string(&header.authorization)
        )?;

        // FILE_SCHEMA — declares the application protocol of this
        // exchange. Default is AP242 (Managed Model-Based 3D
        // Engineering, MIM long-form); legacy AP214 / AP203 paths
        // remain available via [`Self::with_protocol`].
        writeln!(
            self.writer,
            "FILE_SCHEMA(('{}'));",
            self.protocol.schema_name()
        )?;
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
        // Emit the end point too so the LINE's two endpoints are both
        // representable in the file (the importer keys edge extent off
        // the vertices, but downstream readers expect both points).
        let _end_id = self.write_cartesian_point(end)?;

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
            // Rational B-spline curve. Per ISO 10303-42 a weighted
            // B-spline is a *complex* (AND-combined) entity instance,
            // not a standalone `RATIONAL_B_SPLINE_CURVE`. The long-form
            // MIM spelling combines the supertype chain so importers
            // (incl. OCCT) resolve the rational specialisation:
            //
            //   ( BOUNDED_CURVE() B_SPLINE_CURVE(deg,(cps),.UNSPECIFIED.,.F.,.F.)
            //     B_SPLINE_CURVE_WITH_KNOTS((mults),(knots),.UNSPECIFIED.)
            //     CURVE() GEOMETRIC_REPRESENTATION_ITEM()
            //     RATIONAL_B_SPLINE_CURVE((weights))
            //     REPRESENTATION_ITEM('') )
            #[allow(clippy::expect_used)]
            // Reason: weights.is_some() is checked one line above in the
            // enclosing `if` guard; the Option cannot be None here.
            let weights_slice =
                weights.expect("weights.is_some() verified by enclosing `if` guard");
            writeln!(self.writer,
                "{id}=( BOUNDED_CURVE() B_SPLINE_CURVE({degree},({cps}),.UNSPECIFIED.,.F.,.F.) B_SPLINE_CURVE_WITH_KNOTS(({mults}),({knots}),.UNSPECIFIED.) CURVE() GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_CURVE(({weights})) REPRESENTATION_ITEM('') );",
                id = id,
                degree = degree,
                cps = format_id_list(&cp_ids),
                mults = format_int_list(multiplicities),
                knots = format_real_list(knots),
                weights = format_real_list(weights_slice),
            )?;
        } else {
            // Non-rational B-spline. The leading `.` on the final
            // `.UNSPECIFIED.` enum was previously missing — that made
            // every B-spline curve unparseable.
            writeln!(self.writer,
                "{}=B_SPLINE_CURVE_WITH_KNOTS({},({}),.UNSPECIFIED.,.F.,.F.,({}),({}),.UNSPECIFIED.);",
                id,
                degree,
                format_id_list(&cp_ids),
                format_int_list(multiplicities),
                format_real_list(knots)
            )?;
        }

        Ok(id)
    }

    /// Write a curve entity.
    ///
    /// `_vertex_map` is unused: curve geometry carries its own
    /// coordinates in the snapshot, independent of the vertex topology.
    pub fn write_curve(
        &mut self,
        curve: &crate::formats::ros_snapshot::CurveData,
        _vertex_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        use crate::formats::ros_snapshot::CurveData;

        match curve {
            CurveData::Line { start, end } => {
                // LINE('', point, vector). The point is the line origin;
                // the vector is the (unit) direction with magnitude 1.
                let start_id = self.write_cartesian_point(start)?;
                let raw = [end[0] - start[0], end[1] - start[1], end[2] - start[2]];
                let len = (raw[0] * raw[0] + raw[1] * raw[1] + raw[2] * raw[2]).sqrt();
                let unit = if len > 0.0 {
                    [raw[0] / len, raw[1] / len, raw[2] / len]
                } else {
                    [1.0, 0.0, 0.0]
                };
                let dir_id = self.write_direction(&unit)?;
                let vec_id = self.write_vector(dir_id, 1.0)?;
                let id = self.next_id();
                writeln!(self.writer, "{}=LINE('',{},{});", id, start_id, vec_id)?;
                Ok(id)
            }
            CurveData::Circle {
                center,
                radius,
                normal,
            } => {
                // write_circle emits the placement + CIRCLE itself.
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
                start_angle: _,
                end_angle: _,
            } => {
                // In an ADVANCED_BREP an arc edge references its
                // *unbounded* basis CIRCLE as `edge_geometry`; the arc
                // extent is defined by the EDGE_CURVE's two VERTEX_POINTs
                // (the CAx-IF recommended-practice form, and exactly what
                // the importer reconstructs). A standalone TRIMMED_CURVE
                // is a wireframe/geometric-set construct that B-Rep
                // edge-geometry resolvers (including ours and OCCT's edge
                // path) do not accept here, so emit the basis circle.
                self.write_circle(
                    &[center[0], center[1], center[2]],
                    &[normal[0], normal[1], normal[2]],
                    *radius,
                )
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

                // CONICAL_SURFACE('', placement, radius, semi_angle).
                // The angle unit declared in the geometric context is
                // RADIAN, so the semi-angle MUST be emitted in radians
                // (it was previously degrees → wrong cone half-angle and
                // an unreadable cross-section). We place the reference
                // frame at the apex with `radius = 0`, which the importer
                // reads back as apex == placement origin (ISO 10303-42:
                // apex = origin − axis·(radius/tan(semi_angle)), and with
                // radius == 0 the offset vanishes).
                let id = self.next_id();
                writeln!(
                    self.writer,
                    "{}=CONICAL_SURFACE('',{},0.0,{:.12});",
                    id, axis_placement, half_angle
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
            SurfaceData::BSpline {
                control_points,
                knots_u,
                knots_v,
                degree_u,
                degree_v,
            } => self.write_b_spline_surface(
                *degree_u,
                *degree_v,
                control_points,
                knots_u,
                knots_v,
                None,
            ),
            SurfaceData::Nurbs {
                control_points,
                weights,
                knots_u,
                knots_v,
                degree_u,
                degree_v,
            } => self.write_b_spline_surface(
                *degree_u,
                *degree_v,
                control_points,
                knots_u,
                knots_v,
                Some(weights),
            ),
        }
    }

    /// Write a B-spline (or rational/NURBS) surface as
    /// `B_SPLINE_SURFACE_WITH_KNOTS`. When `weights` is supplied and any
    /// weight differs from 1, the rational complex-entity form is
    /// emitted per ISO 10303-42. Control points arrive in row-major
    /// `[u][v]` order; the knot vectors are full (clamped) vectors which
    /// we collapse into the `(distinct_knots, multiplicities)` pairs the
    /// schema (and the importer) expect.
    fn write_b_spline_surface(
        &mut self,
        degree_u: u32,
        degree_v: u32,
        control_points: &[Vec<[f64; 3]>],
        knots_u: &[f64],
        knots_v: &[f64],
        weights: Option<&[Vec<f64>]>,
    ) -> std::io::Result<StepId> {
        // Emit the control-point grid as CARTESIAN_POINTs and collect a
        // list-of-lists of their ids in [u][v] order.
        let mut row_id_lists: Vec<String> = Vec::with_capacity(control_points.len());
        for row in control_points {
            let mut ids = Vec::with_capacity(row.len());
            for cp in row {
                ids.push(self.write_cartesian_point(cp)?);
            }
            row_id_lists.push(format!("({})", format_id_list(&ids)));
        }
        let cp_grid = row_id_lists.join(",");

        let (u_knots, u_mults) = collapse_knots(knots_u);
        let (v_knots, v_mults) = collapse_knots(knots_v);

        let rational = weights
            .map(|w| {
                w.iter()
                    .any(|row| row.iter().any(|&x| (x - 1.0).abs() > 1e-12))
            })
            .unwrap_or(false);

        let id = self.next_id();
        if rational {
            #[allow(clippy::expect_used)]
            // Reason: `rational` is only true when weights.is_some().
            let w = weights.expect("rational implies weights present");
            let weight_grid = w
                .iter()
                .map(|row| format!("({})", format_real_list(row)))
                .collect::<Vec<_>>()
                .join(",");
            writeln!(self.writer,
                "{id}=( BOUNDED_SURFACE() B_SPLINE_SURFACE({du},{dv},({cps}),.UNSPECIFIED.,.F.,.F.,.F.) B_SPLINE_SURFACE_WITH_KNOTS(({um}),({vm}),({uk}),({vk}),.UNSPECIFIED.) GEOMETRIC_REPRESENTATION_ITEM() RATIONAL_B_SPLINE_SURFACE(({weights})) REPRESENTATION_ITEM('') SURFACE() );",
                id = id,
                du = degree_u,
                dv = degree_v,
                cps = cp_grid,
                um = format_int_list(&u_mults),
                vm = format_int_list(&v_mults),
                uk = format_real_list(&u_knots),
                vk = format_real_list(&v_knots),
                weights = weight_grid,
            )?;
        } else {
            writeln!(self.writer,
                "{id}=B_SPLINE_SURFACE_WITH_KNOTS('',{du},{dv},({cps}),.UNSPECIFIED.,.F.,.F.,.F.,({um}),({vm}),({uk}),({vk}),.UNSPECIFIED.);",
                id = id,
                du = degree_u,
                dv = degree_v,
                cps = cp_grid,
                um = format_int_list(&u_mults),
                vm = format_int_list(&v_mults),
                uk = format_real_list(&u_knots),
                vk = format_real_list(&v_knots),
            )?;
        }
        Ok(id)
    }

    /// Write a fallback LINE from an already-emitted `CARTESIAN_POINT`
    /// origin. Used only when an edge has no backing curve in the
    /// snapshot; the importer re-derives the line's extent from the two
    /// vertex positions, so the unit-X direction placeholder is inert.
    /// `_end_point_id` is accepted for call-site symmetry.
    fn write_line_from_vertices(
        &mut self,
        start_point_id: StepId,
        _end_point_id: StepId,
    ) -> std::io::Result<StepId> {
        let dir_id = self.write_direction(&[1.0, 0.0, 0.0])?;
        let vec_id = self.write_vector(dir_id, 1.0)?;
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=LINE('',{},{});",
            id, start_point_id, vec_id
        )?;
        Ok(id)
    }

    /// Write an `EDGE_CURVE` entity.
    ///
    /// `vertex_point_map` maps each vertex UUID to an already-emitted
    /// `VERTEX_POINT` id (so vertices are shared, not duplicated per
    /// edge). `vertex_geom_map` maps the same UUID to its underlying
    /// `CARTESIAN_POINT` (used to synthesise a `LINE` when the curve
    /// lookup misses). The edge references its actual backing curve
    /// (line / circle / b-spline) when present.
    fn write_edge_curve(
        &mut self,
        edge: &crate::formats::ros_snapshot::EdgeData,
        vertex_point_map: &HashMap<&uuid::Uuid, StepId>,
        vertex_geom_map: &HashMap<&uuid::Uuid, StepId>,
        curve_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        let v1_id = vertex_point_map
            .get(&edge.start_vertex)
            .copied()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing start vertex")
            })?;
        let v2_id = vertex_point_map
            .get(&edge.end_vertex)
            .copied()
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "Missing end vertex")
            })?;

        // Reference the edge's actual backing curve. Only fall back to a
        // straight LINE when there genuinely is no curve to reference
        // (a degenerate snapshot) — never silently when a curved edge's
        // curve simply wasn't emitted.
        let curve_id = match edge.curve.as_ref().and_then(|c| curve_map.get(c).copied()) {
            Some(id) => id,
            None => {
                let s = vertex_geom_map.get(&edge.start_vertex).copied();
                let e = vertex_geom_map.get(&edge.end_vertex).copied();
                match (s, e) {
                    (Some(s_pt), Some(e_pt)) => self.write_line_from_vertices(s_pt, e_pt)?,
                    _ => {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "edge has neither a curve nor resolvable endpoint geometry",
                        ))
                    }
                }
            }
        };

        let same_sense = if edge.orientation { ".T." } else { ".F." };
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=EDGE_CURVE('',{},{},{},{});",
            id, v1_id, v2_id, curve_id, same_sense
        )?;
        Ok(id)
    }

    /// Write an `ADVANCED_FACE` and the topology it owns.
    ///
    /// Each loop is materialised as an `EDGE_LOOP` of `ORIENTED_EDGE`s
    /// (with per-edge orientation taken from the loop's `orientations`),
    /// wrapped in a `FACE_OUTER_BOUND` (outer) or `FACE_BOUND` (inner).
    fn write_advanced_face(
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

        let mut bound_ids = Vec::new();

        if let Some(outer_loop_uuid) = &face.outer_loop {
            if let Some(loop_data) = loop_map.get(outer_loop_uuid) {
                if let Some(bound_id) = self.write_face_bound(loop_data, edge_map, true)? {
                    bound_ids.push(bound_id);
                }
            }
        }

        for inner_loop_uuid in &face.inner_loops {
            if let Some(loop_data) = loop_map.get(inner_loop_uuid) {
                if let Some(bound_id) = self.write_face_bound(loop_data, edge_map, false)? {
                    bound_ids.push(bound_id);
                }
            }
        }

        if bound_ids.is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "face has no resolvable bounds",
            ));
        }

        // ADVANCED_FACE same_sense follows the face's stored orientation.
        let same_sense = if face.orientation { ".T." } else { ".F." };
        let id = self.next_id();
        writeln!(
            self.writer,
            "{}=ADVANCED_FACE('',({}),{},{});",
            id,
            format_id_list(&bound_ids),
            surface_id,
            same_sense
        )?;
        Ok(id)
    }

    /// Materialise one loop as `ORIENTED_EDGE`s → `EDGE_LOOP` →
    /// `FACE_OUTER_BOUND`/`FACE_BOUND`. Returns `None` when no edge in
    /// the loop resolves (a degenerate loop is dropped rather than
    /// emitting an empty `EDGE_LOOP`, which the schema forbids).
    fn write_face_bound(
        &mut self,
        loop_data: &crate::formats::ros_snapshot::LoopData,
        edge_map: &HashMap<&uuid::Uuid, StepId>,
        is_outer: bool,
    ) -> std::io::Result<Option<StepId>> {
        let mut oriented_ids = Vec::with_capacity(loop_data.edges.len());
        for (i, edge_uuid) in loop_data.edges.iter().enumerate() {
            let Some(edge_id) = edge_map.get(edge_uuid).copied() else {
                continue;
            };
            let forward = loop_data.orientations.get(i).copied().unwrap_or(true);
            let oriented_id = self.next_id();
            writeln!(
                self.writer,
                "{}=ORIENTED_EDGE('',*,*,{},{});",
                oriented_id,
                edge_id,
                if forward { ".T." } else { ".F." }
            )?;
            oriented_ids.push(oriented_id);
        }
        if oriented_ids.is_empty() {
            return Ok(None);
        }

        let loop_id = self.next_id();
        writeln!(
            self.writer,
            "{}=EDGE_LOOP('',({}));",
            loop_id,
            format_id_list(&oriented_ids)
        )?;

        let bound_id = self.next_id();
        if is_outer {
            writeln!(
                self.writer,
                "{}=FACE_OUTER_BOUND('',{},.T.);",
                bound_id, loop_id
            )?;
        } else {
            writeln!(self.writer, "{}=FACE_BOUND('',{},.T.);", bound_id, loop_id)?;
        }
        Ok(Some(bound_id))
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

    /// Write the solid B-Rep entity for `solid`, returning its id.
    ///
    /// The first shell is the outer boundary. If the solid carries
    /// additional (inner) shells they are voids: a `BREP_WITH_VOIDS`
    /// (with `ORIENTED_CLOSED_SHELL` wrappers) is emitted instead of a
    /// plain `MANIFOLD_SOLID_BREP`. The solid `name` is carried into the
    /// entity label.
    fn write_solid_brep(
        &mut self,
        solid: &crate::formats::ros_snapshot::SolidData,
        shell_map: &HashMap<&uuid::Uuid, StepId>,
    ) -> std::io::Result<StepId> {
        let mut shell_ids = solid
            .shells
            .iter()
            .filter_map(|s| shell_map.get(s).copied());
        let outer = shell_ids.next().ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Solid has no shells")
        })?;
        let voids: Vec<StepId> = shell_ids.collect();

        let label = solid
            .feature_type
            .as_deref()
            .map(escape_step_string)
            .unwrap_or_default();

        let id = self.next_id();
        if voids.is_empty() {
            writeln!(
                self.writer,
                "{}=MANIFOLD_SOLID_BREP('{}',{});",
                id, label, outer
            )?;
        } else {
            // Each void shell is wrapped in an ORIENTED_CLOSED_SHELL
            // oriented inward (.F.) per ISO 10303-42 BREP_WITH_VOIDS.
            let mut oriented_voids = Vec::with_capacity(voids.len());
            for v in voids {
                let ov = self.next_id();
                writeln!(self.writer, "{}=ORIENTED_CLOSED_SHELL('',{},.F.);", ov, v)?;
                oriented_voids.push(ov);
            }
            writeln!(
                self.writer,
                "{}=BREP_WITH_VOIDS('{}',{},({}));",
                id,
                label,
                outer,
                format_id_list(&oriented_voids)
            )?;
        }
        Ok(id)
    }

    /// Emit the PRODUCT structure chain that lets a STEP importer locate
    /// the shape: `PRODUCT` → `PRODUCT_DEFINITION_FORMATION` →
    /// `PRODUCT_DEFINITION` → `PRODUCT_DEFINITION_SHAPE` →
    /// `SHAPE_DEFINITION_REPRESENTATION` bound to `shape_rep_id`. A
    /// minimal `APPLICATION_CONTEXT` / `PRODUCT_DEFINITION_CONTEXT`
    /// scaffold is shared per call. Without this chain mainstream CAD
    /// imports the file as empty.
    fn write_product_structure(&mut self, name: &str, shape_rep_id: StepId) -> std::io::Result<()> {
        let label = escape_step_string(name);

        let app_ctx = self.next_id();
        writeln!(
            self.writer,
            "{}=APPLICATION_CONTEXT('managed model based 3d engineering');",
            app_ctx
        )?;
        let app_proto = self.next_id();
        writeln!(
            self.writer,
            "{}=APPLICATION_PROTOCOL_DEFINITION('international standard','ap242_managed_model_based_3d_engineering',2020,{});",
            app_proto, app_ctx
        )?;
        let prod_ctx = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT_CONTEXT('',{},'mechanical');",
            prod_ctx, app_ctx
        )?;
        let prod_def_ctx = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT_DEFINITION_CONTEXT('part definition',{},'design');",
            prod_def_ctx, app_ctx
        )?;

        let product = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT('{}','{}','',({}));",
            product, label, label, prod_ctx
        )?;
        let formation = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT_DEFINITION_FORMATION('','',{});",
            formation, product
        )?;
        let prod_def = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT_DEFINITION('design','',{},{});",
            prod_def, formation, prod_def_ctx
        )?;
        let pds = self.next_id();
        writeln!(
            self.writer,
            "{}=PRODUCT_DEFINITION_SHAPE('','',{});",
            pds, prod_def
        )?;
        let sdr = self.next_id();
        writeln!(
            self.writer,
            "{}=SHAPE_DEFINITION_REPRESENTATION({},{});",
            sdr, pds, shape_rep_id
        )?;
        Ok(())
    }

    /// Write a geometric representation context (required by ADVANCED_BREP_SHAPE_REPRESENTATION)
    fn write_geometric_context(&mut self) -> std::io::Result<StepId> {
        // Length unit: millimeters
        let mm_id = self.next_id();
        writeln!(
            self.writer,
            "{}=( LENGTH_UNIT() NAMED_UNIT(*) SI_UNIT(.MILLI.,.METRE.) );",
            mm_id
        )?;

        // Angle unit: radians
        let rad_id = self.next_id();
        writeln!(
            self.writer,
            "{}=( NAMED_UNIT(*) PLANE_ANGLE_UNIT() SI_UNIT($,.RADIAN.) );",
            rad_id
        )?;

        // Solid angle unit: steradians
        let sr_id = self.next_id();
        writeln!(
            self.writer,
            "{}=( NAMED_UNIT(*) SI_UNIT($,.STERADIAN.) SOLID_ANGLE_UNIT() );",
            sr_id
        )?;

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

    /// Emit a complete B-Rep model into the DATA section: geometry,
    /// topology, solids, an `ADVANCED_BREP_SHAPE_REPRESENTATION`
    /// (sharing `context_id`), and the PRODUCT structure that anchors
    /// the shape. Returns the number of solids written.
    ///
    /// `context_id` is allocated once by the caller (via
    /// [`Self::write_geometric_context`]) and shared across every solid
    /// in the file, as the audit requires.
    fn write_brep_model(
        &mut self,
        snapshot: &BRepSnapshot,
        context_id: StepId,
        model_name: &str,
    ) -> std::io::Result<usize> {
        // Vertices: one CARTESIAN_POINT + one VERTEX_POINT each, shared
        // by every edge that references the vertex.
        let mut vertex_geom_map: HashMap<&Uuid, StepId> = HashMap::new();
        let mut vertex_point_map: HashMap<&Uuid, StepId> = HashMap::new();
        for (vid, vertex) in &snapshot.vertices {
            let pt = self.write_cartesian_point(&vertex.position)?;
            vertex_geom_map.insert(vid, pt);
            let vp = self.next_id();
            writeln!(self.writer, "{}=VERTEX_POINT('',{});", vp, pt)?;
            vertex_point_map.insert(vid, vp);
        }

        // Curves.
        let mut curve_map: HashMap<&Uuid, StepId> = HashMap::new();
        for (cid, curve) in &snapshot.curves {
            let step_id = self.write_curve(curve, &vertex_geom_map)?;
            curve_map.insert(cid, step_id);
        }

        // Surfaces.
        let mut surface_map: HashMap<&Uuid, StepId> = HashMap::new();
        for (sid, surface) in &snapshot.surfaces {
            let step_id = self.write_surface(surface)?;
            surface_map.insert(sid, step_id);
        }

        // Edges (EDGE_CURVE).
        let mut edge_map: HashMap<&Uuid, StepId> = HashMap::new();
        for (eid, edge) in &snapshot.edges {
            let step_id =
                self.write_edge_curve(edge, &vertex_point_map, &vertex_geom_map, &curve_map)?;
            edge_map.insert(eid, step_id);
        }

        let loop_map: HashMap<&Uuid, &crate::formats::ros_snapshot::LoopData> =
            snapshot.loops.iter().map(|(id, data)| (id, data)).collect();

        // Faces (ADVANCED_FACE + bounds/loops/oriented-edges).
        let mut face_map: HashMap<&Uuid, StepId> = HashMap::new();
        for (fid, face) in &snapshot.faces {
            let step_id = self.write_advanced_face(face, &surface_map, &edge_map, &loop_map)?;
            face_map.insert(fid, step_id);
        }

        // Shells.
        let mut shell_map: HashMap<&Uuid, StepId> = HashMap::new();
        for (sid, shell) in &snapshot.shells {
            let step_id = self.write_shell(shell, &face_map)?;
            shell_map.insert(sid, step_id);
        }

        // Solids → ADVANCED_BREP_SHAPE_REPRESENTATION items + product
        // structure. Every solid in the file shares one context.
        let mut solid_ids = Vec::with_capacity(snapshot.solids.len());
        for (_solid_id, solid) in &snapshot.solids {
            let brep_id = self.write_solid_brep(solid, &shell_map)?;
            solid_ids.push(brep_id);
        }
        let solid_count = solid_ids.len();

        if !solid_ids.is_empty() {
            // World-origin placement, listed alongside the solids as a
            // representation item (the importer treats it as the origin).
            let origin = self.write_axis2_placement_3d(
                &[0.0, 0.0, 0.0],
                Some(&[0.0, 0.0, 1.0]),
                Some(&[1.0, 0.0, 0.0]),
            )?;
            let mut items = solid_ids.clone();
            items.push(origin);

            let shape_id = self.next_id();
            writeln!(
                self.writer,
                "{}=ADVANCED_BREP_SHAPE_REPRESENTATION('{}',({}),{});",
                shape_id,
                escape_step_string(model_name),
                format_id_list(&items),
                context_id
            )?;

            self.write_product_structure(model_name, shape_id)?;
        }

        Ok(solid_count)
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

/// Escape a Rust string for inclusion inside a STEP single-quoted
/// string literal. ISO 10303-21 §6.3.2 escapes an embedded apostrophe
/// by doubling it (`''`). Backslashes are control-directive
/// introducers in P21; we double them defensively so a stray `\` in a
/// model name can't open a malformed `\X\`-style directive.
fn escape_step_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('\'', "''")
}

/// Collapse a full (clamped) knot vector into the
/// `(distinct_knots, multiplicities)` pair that
/// `B_SPLINE_CURVE_WITH_KNOTS` / `B_SPLINE_SURFACE_WITH_KNOTS` carry.
/// The kernel and the importer both store knots in expanded form; the
/// STEP schema stores the distinct values with their multiplicities.
fn collapse_knots(knots: &[f64]) -> (Vec<f64>, Vec<u32>) {
    let mut distinct: Vec<f64> = Vec::new();
    let mut mults: Vec<u32> = Vec::new();
    for &k in knots {
        match distinct.last() {
            Some(&last) if (k - last).abs() < 1e-10 => {
                if let Some(m) = mults.last_mut() {
                    *m += 1;
                }
            }
            _ => {
                distinct.push(k);
                mults.push(1u32);
            }
        }
    }
    (distinct, mults)
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

    // Convert to snapshot for easier iteration.
    let snapshot = BRepSnapshot::from_model(model);

    // One geometric_representation_context shared across every solid.
    let context_id = writer
        .write_geometric_context()
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to write geometric context: {}", e),
        })?;

    let model_name = snapshot
        .solids
        .iter()
        .find_map(|(_, s)| s.feature_type.clone())
        .unwrap_or_else(|| "Roshera Model".to_string());

    writer
        .write_brep_model(&snapshot, context_id, &model_name)
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to write B-Rep model: {}", e),
        })?;

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

    // One shared geometric_representation_context for the whole file.
    let context_id = writer
        .write_geometric_context()
        .map_err(|e| ExportError::ExportFailed {
            reason: format!("Failed to write geometric context: {}", e),
        })?;

    // Export each component's geometry as a complete, anchored B-Rep
    // model (geometry + topology + solids + product structure).
    for (idx, component) in assembly.components().enumerate() {
        if !component.properties.suppressed {
            let snapshot = BRepSnapshot::from_model(&component.part);
            let comp_name = snapshot
                .solids
                .iter()
                .find_map(|(_, s)| s.feature_type.clone())
                .unwrap_or_else(|| format!("{}_component_{}", assembly.name, idx));
            writer
                .write_brep_model(&snapshot, context_id, &comp_name)
                .map_err(|e| ExportError::ExportFailed {
                    reason: format!("Failed to write assembly component: {}", e),
                })?;
        }
    }

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

/// STEP export options
#[derive(Debug, Clone)]
pub struct StepExportOptions {
    /// Application protocol to use (AP242, AP214, or AP203). Defaults
    /// to AP242 — Roshera's canonical export protocol.
    pub application_protocol: StepApplicationProtocol,
    /// Include color information
    pub include_colors: bool,
    /// Include layer information
    pub include_layers: bool,
    /// Tolerance for geometric operations
    pub tolerance: f64,
}

/// STEP application protocols.
///
/// AP242 is the canonical Roshera export target. AP214 and AP203 are
/// retained for compatibility with downstream tools that have not
/// migrated yet; new code should leave the default in place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepApplicationProtocol {
    /// AP203 — Configuration Controlled Design (legacy mechanical).
    AP203,
    /// AP214 — Core Data for Automotive Mechanical Design Processes
    /// (legacy automotive).
    AP214,
    /// AP242 — Managed Model-Based 3D Engineering, MIM long-form.
    /// The default for all new exports.
    AP242,
}

impl StepApplicationProtocol {
    /// Schema name written into the STEP `FILE_SCHEMA` header for
    /// this protocol. Matches the canonical short-form schema
    /// identifier emitted by mainstream CAD systems.
    pub fn schema_name(self) -> &'static str {
        match self {
            Self::AP203 => "CONFIG_CONTROL_DESIGN",
            Self::AP214 => "AUTOMOTIVE_DESIGN",
            Self::AP242 => "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF",
        }
    }
}

impl Default for StepApplicationProtocol {
    fn default() -> Self {
        // AP242 is Roshera's canonical export protocol.
        Self::AP242
    }
}

impl Default for StepExportOptions {
    fn default() -> Self {
        Self {
            application_protocol: StepApplicationProtocol::default(),
            include_colors: true,
            include_layers: true,
            tolerance: 1e-6,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::formats::ros_snapshot::{CurveData, SurfaceData};

    /// Helper: drive a StepWriter against an in-memory Vec<u8>, run
    /// the closure, and return the resulting STEP text.
    fn write_into<F>(f: F) -> String
    where
        F: FnOnce(&mut StepWriter<Vec<u8>>) -> std::io::Result<()>,
    {
        let buf: Vec<u8> = Vec::new();
        let mut w = StepWriter::new(buf);
        f(&mut w).expect("writer closure failed");
        w.flush().expect("flush failed");
        // Recover the inner Vec<u8> from the BufWriter.
        let inner = w.writer.into_inner().expect("BufWriter::into_inner failed");
        String::from_utf8(inner).expect("non-UTF8 STEP output")
    }

    // ─── A. Header & ID basics ─────────────────────────────────────

    #[test]
    fn step_header_default_fields() {
        let h = StepHeader::default();
        assert_eq!(h.description, "Roshera CAD Model");
        assert_eq!(h.implementation_level, "2;1");
        assert_eq!(h.name, "model.step");
        assert_eq!(h.author, "Roshera User");
        assert_eq!(h.organization, "Roshera CAD");
        assert_eq!(h.preprocessor_version, "Roshera STEP Processor 1.0");
        assert_eq!(h.originating_system, "Roshera CAD System");
        assert_eq!(h.authorization, "");
    }

    #[test]
    fn step_header_is_mutable() {
        let mut h = StepHeader::default();
        h.name = "custom.step".to_string();
        h.description = "Custom desc".to_string();
        assert_eq!(h.name, "custom.step");
        assert_eq!(h.description, "Custom desc");
    }

    #[test]
    fn step_id_display_formats_as_hash_n() {
        assert_eq!(format!("{}", StepId(1)), "#1");
        assert_eq!(format!("{}", StepId(42)), "#42");
        assert_eq!(format!("{}", StepId(1_000_000)), "#1000000");
    }

    #[test]
    fn step_id_equality_and_hash() {
        let mut map: HashMap<StepId, &str> = HashMap::new();
        map.insert(StepId(7), "seven");
        assert_eq!(map.get(&StepId(7)), Some(&"seven"));
        assert_eq!(map.get(&StepId(8)), None);
        assert_eq!(StepId(3), StepId(3));
        assert_ne!(StepId(3), StepId(4));
    }

    #[test]
    fn step_writer_new_starts_counter_at_one() {
        // First emitted entity must be `#1=...`.
        let out = write_into(|w| {
            w.write_cartesian_point(&[0.0, 0.0, 0.0])?;
            Ok(())
        });
        assert!(out.starts_with("#1=CARTESIAN_POINT"), "got: {}", out);
    }

    #[test]
    fn write_header_emits_iso_marker_and_sections() {
        let out = write_into(|w| {
            let h = StepHeader::default();
            w.write_header(&h)
        });
        assert!(out.contains("ISO-10303-21;"));
        assert!(out.contains("HEADER;"));
        assert!(out.contains("FILE_DESCRIPTION"));
        assert!(out.contains("Roshera CAD Model"));
        assert!(out.contains("FILE_NAME"));
        // Default protocol is AP242 — see StepApplicationProtocol::default.
        assert!(
            out.contains("FILE_SCHEMA(('AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF'))"),
            "default writer must declare AP242, got: {out}"
        );
        assert!(out.contains("ENDSEC;"));
    }

    #[test]
    fn write_header_honours_legacy_ap214_protocol() {
        // `with_protocol` overrides the AP242 default. Used when
        // round-tripping with vendors that have not migrated yet.
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = StepWriter::with_protocol(&mut buf, StepApplicationProtocol::AP214);
            let h = StepHeader::default();
            w.write_header(&h).expect("AP214 header write must succeed");
        }
        let out = String::from_utf8(buf).expect("STEP output must be UTF-8");
        assert!(
            out.contains("FILE_SCHEMA(('AUTOMOTIVE_DESIGN'))"),
            "AP214 writer must declare AUTOMOTIVE_DESIGN, got: {out}"
        );
    }

    #[test]
    fn write_header_honours_legacy_ap203_protocol() {
        let mut buf: Vec<u8> = Vec::new();
        {
            let mut w = StepWriter::with_protocol(&mut buf, StepApplicationProtocol::AP203);
            let h = StepHeader::default();
            w.write_header(&h).expect("AP203 header write must succeed");
        }
        let out = String::from_utf8(buf).expect("STEP output must be UTF-8");
        assert!(
            out.contains("FILE_SCHEMA(('CONFIG_CONTROL_DESIGN'))"),
            "AP203 writer must declare CONFIG_CONTROL_DESIGN, got: {out}"
        );
    }

    // ─── B. Section markers ────────────────────────────────────────

    #[test]
    fn begin_data_emits_data_keyword() {
        let out = write_into(|w| w.begin_data());
        assert_eq!(out.trim(), "DATA;");
    }

    #[test]
    fn end_data_emits_endsec() {
        let out = write_into(|w| w.end_data());
        assert_eq!(out.trim(), "ENDSEC;");
    }

    #[test]
    fn write_end_emits_iso_terminator() {
        let out = write_into(|w| w.write_end());
        assert_eq!(out.trim(), "END-ISO-10303-21;");
    }

    #[test]
    fn flush_succeeds_on_empty_writer() {
        let buf: Vec<u8> = Vec::new();
        let mut w = StepWriter::new(buf);
        w.flush().expect("flush should succeed");
    }

    // ─── C. Primitive entity writers ───────────────────────────────

    #[test]
    fn write_cartesian_point_format() {
        let out = write_into(|w| {
            w.write_cartesian_point(&[1.0, 2.0, 3.0])?;
            Ok(())
        });
        assert!(out.contains("CARTESIAN_POINT('',(1.000000,2.000000,3.000000))"));
        assert!(out.contains("#1="));
    }

    #[test]
    fn write_cartesian_point_negative_coords_six_decimals() {
        let out = write_into(|w| {
            w.write_cartesian_point(&[-1.5, 0.0, -2.25])?;
            Ok(())
        });
        assert!(out.contains("-1.500000"));
        assert!(out.contains("0.000000"));
        assert!(out.contains("-2.250000"));
    }

    #[test]
    fn write_cartesian_point_increments_id() {
        let out = write_into(|w| {
            w.write_cartesian_point(&[0.0, 0.0, 0.0])?;
            w.write_cartesian_point(&[1.0, 1.0, 1.0])?;
            w.write_cartesian_point(&[2.0, 2.0, 2.0])?;
            Ok(())
        });
        assert!(out.contains("#1=CARTESIAN_POINT"));
        assert!(out.contains("#2=CARTESIAN_POINT"));
        assert!(out.contains("#3=CARTESIAN_POINT"));
    }

    #[test]
    fn write_direction_format() {
        let out = write_into(|w| {
            w.write_direction(&[0.0, 0.0, 1.0])?;
            Ok(())
        });
        assert!(out.contains("DIRECTION('',(0.000000,0.000000,1.000000))"));
    }

    #[test]
    fn write_vector_format_includes_direction_ref_and_magnitude() {
        let out = write_into(|w| {
            let dir = w.write_direction(&[1.0, 0.0, 0.0])?;
            w.write_vector(dir, 5.0)?;
            Ok(())
        });
        // direction is #1, vector is #2 referencing #1 with magnitude 5
        assert!(out.contains("#2=VECTOR('',#1,5)"));
    }

    #[test]
    fn write_axis2_placement_3d_with_no_axis_or_ref_dir() {
        let out = write_into(|w| {
            w.write_axis2_placement_3d(&[0.0, 0.0, 0.0], None, None)?;
            Ok(())
        });
        // Origin → #1, AXIS2_PLACEMENT_3D → #2 with $,$
        assert!(out.contains("AXIS2_PLACEMENT_3D('',#1,$,$)"));
    }

    #[test]
    fn write_axis2_placement_3d_full() {
        let out = write_into(|w| {
            w.write_axis2_placement_3d(
                &[0.0, 0.0, 0.0],
                Some(&[0.0, 0.0, 1.0]),
                Some(&[1.0, 0.0, 0.0]),
            )?;
            Ok(())
        });
        // origin #1, axis #2, ref_dir #3, placement #4
        assert!(out.contains("AXIS2_PLACEMENT_3D('',#1,#2,#3)"));
    }

    #[test]
    fn write_axis2_placement_3d_axis_only() {
        let out = write_into(|w| {
            w.write_axis2_placement_3d(&[0.0, 0.0, 0.0], Some(&[0.0, 0.0, 1.0]), None)?;
            Ok(())
        });
        assert!(out.contains("AXIS2_PLACEMENT_3D('',#1,#2,$)"));
    }

    #[test]
    fn write_line_emits_endpoints_direction_vector_line() {
        let out = write_into(|w| {
            w.write_line(&[0.0, 0.0, 0.0], &[3.0, 4.0, 0.0])?;
            Ok(())
        });
        assert!(out.contains("CARTESIAN_POINT")); // both endpoints
        assert!(out.contains("DIRECTION"));
        assert!(out.contains("VECTOR"));
        assert!(out.contains("LINE("));
        // Magnitude of (3,4,0) = 5
        assert!(out.contains(",5)"));
    }

    #[test]
    fn write_circle_includes_radius() {
        let out = write_into(|w| {
            w.write_circle(&[0.0, 0.0, 0.0], &[0.0, 0.0, 1.0], 7.5)?;
            Ok(())
        });
        assert!(out.contains("CIRCLE("));
        assert!(out.contains(",7.5)"));
    }

    // ─── D. B-Spline curves ────────────────────────────────────────

    #[test]
    fn write_b_spline_curve_non_rational() {
        let cps = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 1.0, 0.0]];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let mults = vec![3, 3];
        let out = write_into(|w| {
            w.write_b_spline_curve(2, &cps, &knots, &mults, false, None)?;
            Ok(())
        });
        assert!(out.contains("B_SPLINE_CURVE_WITH_KNOTS"));
        // 3 control points + 1 b-spline entity = 4 entities
        assert!(out.contains("#1=CARTESIAN_POINT"));
        assert!(out.contains("#2=CARTESIAN_POINT"));
        assert!(out.contains("#3=CARTESIAN_POINT"));
        assert!(out.contains("#4=B_SPLINE_CURVE_WITH_KNOTS"));
        // Degree 2 appears as first arg
        assert!(out.contains("B_SPLINE_CURVE_WITH_KNOTS(2,"));
    }

    #[test]
    fn write_b_spline_curve_rational_with_weights() {
        let cps = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 1.0, 0.0]];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let mults = vec![3, 3];
        let weights = vec![1.0, 0.5, 1.0];
        let out = write_into(|w| {
            w.write_b_spline_curve(2, &cps, &knots, &mults, true, Some(&weights))?;
            Ok(())
        });
        assert!(out.contains("RATIONAL_B_SPLINE_CURVE"));
        assert!(out.contains("0.500000"));
    }

    #[test]
    fn write_b_spline_curve_rational_without_weights_falls_back() {
        // The rational branch is only taken when BOTH rational==true AND
        // weights.is_some(). Passing rational=true with weights=None must
        // therefore land in the non-rational branch (no panic).
        let cps = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let mults = vec![2, 2];
        let out = write_into(|w| {
            w.write_b_spline_curve(1, &cps, &knots, &mults, true, None)?;
            Ok(())
        });
        assert!(out.contains("B_SPLINE_CURVE_WITH_KNOTS"));
        assert!(!out.contains("RATIONAL_B_SPLINE_CURVE"));
    }

    // ─── E. CurveData dispatch ─────────────────────────────────────

    #[test]
    fn write_curve_line_variant() {
        let out = write_into(|w| {
            let curve = CurveData::Line {
                start: [0.0, 0.0, 0.0],
                end: [1.0, 0.0, 0.0],
            };
            let vmap: HashMap<&Uuid, StepId> = HashMap::new();
            w.write_curve(&curve, &vmap)?;
            Ok(())
        });
        assert!(out.contains("LINE"));
        assert!(out.contains("DIRECTION"));
        assert!(out.contains("VECTOR"));
    }

    #[test]
    fn write_curve_circle_variant() {
        let out = write_into(|w| {
            let curve = CurveData::Circle {
                center: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                radius: 5.0,
            };
            let vmap: HashMap<&Uuid, StepId> = HashMap::new();
            w.write_curve(&curve, &vmap)?;
            Ok(())
        });
        assert!(out.contains("CIRCLE"));
        assert!(out.contains(",5)"));
    }

    #[test]
    fn write_curve_arc_emits_basis_circle() {
        // An arc edge references its unbounded basis CIRCLE; the extent
        // is carried by the EDGE_CURVE vertices, not a TRIMMED_CURVE.
        let out = write_into(|w| {
            let curve = CurveData::Arc {
                center: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
                radius: 1.0,
                start_angle: 0.0,
                end_angle: std::f64::consts::PI,
            };
            let vmap: HashMap<&Uuid, StepId> = HashMap::new();
            w.write_curve(&curve, &vmap)?;
            Ok(())
        });
        assert!(out.contains("CIRCLE"));
        assert!(
            !out.contains("TRIMMED_CURVE"),
            "B-Rep arc edge uses basis circle: {out}"
        );
        assert!(!out.contains("PARAMETER_VALUE"));
    }

    #[test]
    fn write_curve_bspline_variant() {
        let out = write_into(|w| {
            let curve = CurveData::BSpline {
                control_points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 1.0, 0.0]],
                knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                degree: 2,
            };
            let vmap: HashMap<&Uuid, StepId> = HashMap::new();
            w.write_curve(&curve, &vmap)?;
            Ok(())
        });
        assert!(out.contains("B_SPLINE_CURVE_WITH_KNOTS"));
        assert!(!out.contains("RATIONAL_B_SPLINE_CURVE"));
    }

    #[test]
    fn write_curve_nurbs_variant_emits_rational() {
        let out = write_into(|w| {
            let curve = CurveData::Nurbs {
                control_points: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [2.0, 1.0, 0.0]],
                weights: vec![1.0, 0.5, 1.0],
                knots: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
                degree: 2,
            };
            let vmap: HashMap<&Uuid, StepId> = HashMap::new();
            w.write_curve(&curve, &vmap)?;
            Ok(())
        });
        assert!(out.contains("RATIONAL_B_SPLINE_CURVE"));
    }

    // ─── F. SurfaceData dispatch ───────────────────────────────────

    #[test]
    fn write_surface_plane() {
        let out = write_into(|w| {
            let s = SurfaceData::Plane {
                origin: [0.0, 0.0, 0.0],
                normal: [0.0, 0.0, 1.0],
            };
            w.write_surface(&s)?;
            Ok(())
        });
        assert!(out.contains("AXIS2_PLACEMENT_3D"));
        assert!(out.contains("PLANE("));
    }

    #[test]
    fn write_surface_cylinder() {
        let out = write_into(|w| {
            let s = SurfaceData::Cylinder {
                origin: [0.0, 0.0, 0.0],
                axis: [0.0, 0.0, 1.0],
                radius: 3.0,
            };
            w.write_surface(&s)?;
            Ok(())
        });
        assert!(out.contains("CYLINDRICAL_SURFACE"));
        assert!(out.contains(",3)"));
    }

    #[test]
    fn write_surface_sphere() {
        let out = write_into(|w| {
            let s = SurfaceData::Sphere {
                center: [0.0, 0.0, 0.0],
                radius: 2.5,
            };
            w.write_surface(&s)?;
            Ok(())
        });
        assert!(out.contains("SPHERICAL_SURFACE"));
        assert!(out.contains(",2.5)"));
    }

    // (cone semi-angle is now asserted in radians — see
    // `cone_semi_angle_emitted_in_radians` below. The earlier
    // degrees-based assertion was the bug.)

    #[test]
    fn write_surface_torus() {
        let out = write_into(|w| {
            let s = SurfaceData::Torus {
                center: [0.0, 0.0, 0.0],
                axis: [0.0, 0.0, 1.0],
                major_radius: 5.0,
                minor_radius: 1.0,
            };
            w.write_surface(&s)?;
            Ok(())
        });
        assert!(out.contains("TOROIDAL_SURFACE"));
        assert!(out.contains(",5,1)"));
    }

    // ─── G. Format helpers ─────────────────────────────────────────

    #[test]
    fn format_real_list_six_decimals() {
        assert_eq!(format_real_list(&[1.0]), "1.000000");
        assert_eq!(format_real_list(&[1.0, 2.5]), "1.000000,2.500000");
    }

    #[test]
    fn format_real_list_empty() {
        assert_eq!(format_real_list(&[]), "");
    }

    #[test]
    fn format_real_list_negatives() {
        assert_eq!(format_real_list(&[-1.0, 0.0]), "-1.000000,0.000000");
    }

    #[test]
    fn format_int_list_basic() {
        assert_eq!(format_int_list(&[1, 2, 3]), "1,2,3");
        assert_eq!(format_int_list(&[]), "");
        assert_eq!(format_int_list(&[42]), "42");
    }

    #[test]
    fn format_id_list_uses_hash_prefix() {
        assert_eq!(
            format_id_list(&[StepId(1), StepId(2), StepId(3)]),
            "#1,#2,#3"
        );
        assert_eq!(format_id_list(&[]), "");
    }

    // ─── J. StepExportOptions ──────────────────────────────────────

    #[test]
    fn step_export_options_default() {
        let opts = StepExportOptions::default();
        // Roshera's canonical export protocol is AP242.
        assert_eq!(opts.application_protocol, StepApplicationProtocol::AP242);
        assert!(opts.include_colors);
        assert!(opts.include_layers);
        assert_eq!(opts.tolerance, 1e-6);
    }

    #[test]
    fn step_application_protocol_distinct() {
        assert_ne!(
            StepApplicationProtocol::AP203,
            StepApplicationProtocol::AP214
        );
        assert_ne!(
            StepApplicationProtocol::AP214,
            StepApplicationProtocol::AP242
        );
        assert_ne!(
            StepApplicationProtocol::AP203,
            StepApplicationProtocol::AP242
        );
        assert_eq!(
            StepApplicationProtocol::AP203,
            StepApplicationProtocol::AP203
        );
    }

    #[test]
    fn step_application_protocol_default_is_ap242() {
        assert_eq!(
            StepApplicationProtocol::default(),
            StepApplicationProtocol::AP242
        );
    }

    #[test]
    fn step_application_protocol_schema_names() {
        assert_eq!(
            StepApplicationProtocol::AP203.schema_name(),
            "CONFIG_CONTROL_DESIGN"
        );
        assert_eq!(
            StepApplicationProtocol::AP214.schema_name(),
            "AUTOMOTIVE_DESIGN"
        );
        assert_eq!(
            StepApplicationProtocol::AP242.schema_name(),
            "AP242_MANAGED_MODEL_BASED_3D_ENGINEERING_MIM_LF"
        );
    }

    // ─── K. Assembly constraint name mapping ───────────────────────

    #[test]
    fn assembly_constraint_coincident_maps_correctly() {
        use geometry_engine::assembly::MateType;
        let out = write_into(|w| {
            w.write_assembly_constraint(&MateType::Coincident)?;
            Ok(())
        });
        assert!(out.contains("COINCIDENT_CONSTRAINT"));
    }

    #[test]
    fn assembly_constraint_distance_maps_correctly() {
        use geometry_engine::assembly::MateType;
        let out = write_into(|w| {
            w.write_assembly_constraint(&MateType::Distance(10.0))?;
            Ok(())
        });
        assert!(out.contains("DISTANCE_CONSTRAINT"));
    }

    #[test]
    fn assembly_constraint_tangent_maps_correctly() {
        use geometry_engine::assembly::MateType;
        let out = write_into(|w| {
            w.write_assembly_constraint(&MateType::Tangent)?;
            Ok(())
        });
        assert!(out.contains("TANGENT_CONSTRAINT"));
    }

    #[test]
    fn assembly_constraint_lock_maps_correctly() {
        use geometry_engine::assembly::MateType;
        let out = write_into(|w| {
            w.write_assembly_constraint(&MateType::Lock)?;
            Ok(())
        });
        assert!(out.contains("LOCK_CONSTRAINT"));
    }

    #[test]
    fn assembly_constraint_concentric_and_parallel() {
        use geometry_engine::assembly::MateType;
        let out_c = write_into(|w| {
            w.write_assembly_constraint(&MateType::Concentric)?;
            Ok(())
        });
        assert!(out_c.contains("CONCENTRIC_CONSTRAINT"));

        let out_p = write_into(|w| {
            w.write_assembly_constraint(&MateType::Parallel)?;
            Ok(())
        });
        assert!(out_p.contains("PARALLEL_CONSTRAINT"));
    }

    // ─── L. Header & structural fixes ──────────────────────────────

    #[test]
    fn header_records_are_semicolon_terminated() {
        let out = write_into(|w| w.write_header(&StepHeader::default()));
        for key in ["FILE_DESCRIPTION", "FILE_NAME", "FILE_SCHEMA"] {
            let line = out
                .lines()
                .find(|l| l.starts_with(key))
                .unwrap_or_else(|| panic!("{key} present"));
            assert!(
                line.trim_end().ends_with(';'),
                "{key} must end in ;: {line}"
            );
        }
    }

    #[test]
    fn b_spline_curve_non_rational_enum_has_leading_dot() {
        let cps = vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        let knots = vec![0.0, 0.0, 1.0, 1.0];
        let mults = vec![2, 2];
        let out = write_into(|w| {
            w.write_b_spline_curve(1, &cps, &knots, &mults, false, None)?;
            Ok(())
        });
        // The bug was a missing leading `.` on the trailing UNSPECIFIED
        // enum (`UNSPECIFIED.`), which made the record unparseable.
        assert!(out.contains(".UNSPECIFIED.);"), "got: {out}");
        assert!(
            !out.contains(",UNSPECIFIED."),
            "stray bare UNSPECIFIED: {out}"
        );
    }

    #[test]
    fn cone_semi_angle_emitted_in_radians() {
        let out = write_into(|w| {
            let s = SurfaceData::Cone {
                apex: [0.0, 0.0, 0.0],
                axis: [0.0, 0.0, 1.0],
                half_angle: std::f64::consts::FRAC_PI_4,
            };
            w.write_surface(&s)?;
            Ok(())
        });
        assert!(out.contains("CONICAL_SURFACE"));
        // π/4 rad ≈ 0.785398, NOT 45 (degrees).
        assert!(
            out.contains("0.785398"),
            "semi-angle must be radians: {out}"
        );
        assert!(
            !out.contains(",45.0") && !out.contains(",45)"),
            "no degrees: {out}"
        );
    }

    #[test]
    fn nurbs_surface_emits_control_net_not_abstract_surface() {
        let out = write_into(|w| {
            let s = SurfaceData::Nurbs {
                control_points: vec![
                    vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0]],
                    vec![[0.0, 1.0, 0.0], [1.0, 1.0, 1.0]],
                ],
                weights: vec![vec![1.0, 0.5], vec![1.0, 1.0]],
                knots_u: vec![0.0, 0.0, 1.0, 1.0],
                knots_v: vec![0.0, 0.0, 1.0, 1.0],
                degree_u: 1,
                degree_v: 1,
            };
            w.write_surface(&s)?;
            Ok(())
        });
        assert!(out.contains("B_SPLINE_SURFACE_WITH_KNOTS"), "got: {out}");
        assert!(
            out.contains("RATIONAL_B_SPLINE_SURFACE"),
            "weighted → rational: {out}"
        );
        assert!(
            !out.contains("=SURFACE('')"),
            "no abstract SURFACE drop: {out}"
        );
    }

    #[test]
    fn multi_shell_solid_emits_brep_with_voids_and_name() {
        use crate::formats::ros_snapshot::{BRepSnapshot, ShellData, ShellType, SolidData};

        // Two empty closed shells in one solid is enough to exercise the
        // outer-vs-void routing in `write_solid_brep`; no faces are
        // needed (and an empty face list keeps the topology emitters,
        // which would reject an edgeless loop, out of the way).
        let mut snap = BRepSnapshot::new();
        let mk_shell = |snap: &mut BRepSnapshot| -> Uuid {
            let sh = Uuid::new_v4();
            snap.shells.push((
                sh,
                ShellData {
                    faces: vec![],
                    is_closed: true,
                    shell_type: ShellType::Closed,
                },
            ));
            sh
        };
        let outer = mk_shell(&mut snap);
        let inner = mk_shell(&mut snap);
        snap.solids.push((
            Uuid::new_v4(),
            SolidData {
                shells: vec![outer, inner],
                feature_type: Some("hollow_block".to_string()),
            },
        ));

        let out = write_into(|w| {
            let ctx = w.write_geometric_context()?;
            w.write_brep_model(&snap, ctx, "hollow_block")?;
            Ok(())
        });
        assert!(
            out.contains("BREP_WITH_VOIDS"),
            "two-shell solid → voids: {out}"
        );
        assert!(out.contains("ORIENTED_CLOSED_SHELL"), "void wrapper: {out}");
        assert!(out.contains("'hollow_block'"), "solid name carried: {out}");
    }

    #[test]
    fn collapse_knots_groups_multiplicities() {
        let (distinct, mults) = collapse_knots(&[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert_eq!(distinct, vec![0.0, 1.0]);
        assert_eq!(mults, vec![3, 3]);
    }

    #[test]
    fn escape_step_string_doubles_apostrophe() {
        assert_eq!(escape_step_string("O'Brien"), "O''Brien");
        assert_eq!(escape_step_string("a\\b"), "a\\\\b");
    }
}
