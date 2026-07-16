//! Serializable snapshot of B-Rep model for ROS format
//!
//! Since BRepModel uses DashMap for concurrent access, we need a
//! serializable representation for export/import

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::{
    curve::{
        Arc as GeoArc, Circle as GeoCircle, Curve, Line as GeoLine, NurbsCurve as GeoNurbsCurve,
    },
    edge::{Edge, EdgeOrientation},
    face::{Face, FaceOrientation},
    fillet_surfaces::CylindricalFillet,
    r#loop::{Loop, LoopType},
    shell::{Shell, ShellType as GeoShellType},
    solid::Solid,
    surface::{
        Cone as GeoCone, Cylinder as GeoCylinder, GeneralNurbsSurface, Plane as GeoPlane,
        RuledSurface, Sphere as GeoSphere, Surface, SurfaceOfRevolution as GeoSurfaceOfRevolution,
        Torus as GeoTorus,
    },
    topology_builder::BRepModel,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Serializable snapshot of a B-Rep model
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BRepSnapshot {
    /// Vertices with their IDs
    pub vertices: Vec<(Uuid, VertexData)>,

    /// Curves with their IDs  
    pub curves: Vec<(Uuid, CurveData)>,

    /// Edges with their IDs
    pub edges: Vec<(Uuid, EdgeData)>,

    /// Loops with their IDs
    pub loops: Vec<(Uuid, LoopData)>,

    /// Faces with their IDs
    pub faces: Vec<(Uuid, FaceData)>,

    /// Surfaces with their IDs
    pub surfaces: Vec<(Uuid, SurfaceData)>,

    /// Shells with their IDs
    pub shells: Vec<(Uuid, ShellData)>,

    /// Solids with their IDs
    pub solids: Vec<(Uuid, SolidData)>,

    /// Metadata
    pub metadata: BRepMetadata,
}

/// Serializable vertex data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VertexData {
    pub position: [f64; 3],
    pub tolerance: f64,
}

/// Serializable curve data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CurveData {
    Line {
        start: [f64; 3],
        end: [f64; 3],
    },
    Circle {
        center: [f64; 3],
        normal: [f64; 3],
        radius: f64,
    },
    Arc {
        center: [f64; 3],
        normal: [f64; 3],
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    },
    BSpline {
        control_points: Vec<[f64; 3]>,
        knots: Vec<f64>,
        degree: u32,
    },
    Nurbs {
        control_points: Vec<[f64; 3]>,
        weights: Vec<f64>,
        knots: Vec<f64>,
        degree: u32,
    },
}

/// Serializable edge data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeData {
    pub start_vertex: Uuid,
    pub end_vertex: Uuid,
    pub curve: Option<Uuid>,
    pub orientation: bool,
}

/// Serializable loop data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopData {
    pub edges: Vec<Uuid>,
    pub orientations: Vec<bool>,
    pub is_outer: bool,
}

/// Serializable face data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaceData {
    pub surface: Option<Uuid>,
    pub outer_loop: Option<Uuid>,
    pub inner_loops: Vec<Uuid>,
    pub orientation: bool,
}

/// Serializable surface data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SurfaceData {
    Plane {
        origin: [f64; 3],
        normal: [f64; 3],
    },
    Cylinder {
        origin: [f64; 3],
        axis: [f64; 3],
        radius: f64,
    },
    Sphere {
        center: [f64; 3],
        radius: f64,
    },
    Cone {
        apex: [f64; 3],
        axis: [f64; 3],
        half_angle: f64,
    },
    Torus {
        center: [f64; 3],
        axis: [f64; 3],
        major_radius: f64,
        minor_radius: f64,
    },
    BSpline {
        control_points: Vec<Vec<[f64; 3]>>,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        degree_u: u32,
        degree_v: u32,
    },
    Nurbs {
        control_points: Vec<Vec<[f64; 3]>>,
        weights: Vec<Vec<f64>>,
        knots_u: Vec<f64>,
        knots_v: Vec<f64>,
        degree_u: u32,
        degree_v: u32,
    },
    /// A surface of revolution: a profile curve revolved about an axis. Exported
    /// as a STEP `SURFACE_OF_REVOLUTION` (exact, smooth) rather than the degree-1
    /// grid fallback that faceted revolved parts (the FreeCAD nozzle issue).
    SurfaceOfRevolution {
        axis_origin: [f64; 3],
        axis_direction: [f64; 3],
        profile: CurveData,
        angle: f64,
    },
}

/// Serializable shell data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellData {
    pub faces: Vec<Uuid>,
    pub is_closed: bool,
    pub shell_type: ShellType,
}

/// Shell type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ShellType {
    Open,
    Closed,
    Compound,
}

/// Serializable solid data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolidData {
    pub shells: Vec<Uuid>,
    pub feature_type: Option<String>,
}

/// B-Rep metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BRepMetadata {
    /// Creation timestamp
    pub created_at: u64,

    /// Last modified timestamp
    pub modified_at: u64,

    /// Unit of measurement
    pub units: String,

    /// Tolerance value
    pub tolerance: f64,

    /// Additional properties
    pub properties: HashMap<String, serde_json::Value>,
}

impl BRepSnapshot {
    /// Create a new empty snapshot
    pub fn new() -> Self {
        Self {
            vertices: Vec::new(),
            curves: Vec::new(),
            edges: Vec::new(),
            loops: Vec::new(),
            faces: Vec::new(),
            surfaces: Vec::new(),
            shells: Vec::new(),
            solids: Vec::new(),
            metadata: BRepMetadata {
                created_at: ros_format::current_time_ms(),
                modified_at: ros_format::current_time_ms(),
                units: "millimeters".to_string(),
                tolerance: 1e-6,
                properties: HashMap::new(),
            },
        }
    }

    /// Convert from BRepModel to snapshot — extracts all topology and geometry
    pub fn from_model(model: &BRepModel) -> Self {
        let mut snapshot = Self::new();

        // ── Vertices ──
        for (vid, vertex) in model.vertices.iter() {
            let uuid = id_to_uuid(vid as u64);
            snapshot.vertices.push((
                uuid,
                VertexData {
                    position: vertex.position,
                    tolerance: vertex.tolerance,
                },
            ));
        }

        // ── Curves ──
        for (cid, curve) in model.curves.iter() {
            let uuid = id_to_uuid(cid as u64);
            let curve_data = extract_curve_data(curve);
            snapshot.curves.push((uuid, curve_data));
        }

        // ── Edges ──
        for (eid, edge) in model.edges.iter() {
            let uuid = id_to_uuid(eid as u64);
            snapshot.edges.push((
                uuid,
                EdgeData {
                    start_vertex: id_to_uuid(edge.start_vertex as u64),
                    end_vertex: id_to_uuid(edge.end_vertex as u64),
                    curve: Some(id_to_uuid(edge.curve_id as u64)),
                    orientation: matches!(edge.orientation, EdgeOrientation::Forward),
                },
            ));
        }

        // ── Loops ──
        for (lid, loop_) in model.loops.iter() {
            let uuid = id_to_uuid(lid as u64);
            snapshot.loops.push((
                uuid,
                LoopData {
                    edges: loop_
                        .edges
                        .iter()
                        .map(|&eid| id_to_uuid(eid as u64))
                        .collect(),
                    orientations: loop_.orientations.clone(),
                    is_outer: matches!(loop_.loop_type, LoopType::Outer),
                },
            ));
        }

        // ── Surfaces ──
        // SurfaceStore.get(id) is the reliable accessor (iter() depends on type_map)
        for sid in 0..model.surfaces.len() as u32 {
            if let Some(surface) = model.surfaces.get(sid) {
                let uuid = id_to_uuid(sid as u64);
                let surface_data = extract_surface_data(surface);
                snapshot.surfaces.push((uuid, surface_data));
            }
        }

        // ── Faces ──
        for (fid, face) in model.faces.iter() {
            let uuid = id_to_uuid(fid as u64);
            snapshot.faces.push((
                uuid,
                FaceData {
                    surface: Some(id_to_uuid(face.surface_id as u64)),
                    outer_loop: Some(id_to_uuid(face.outer_loop as u64)),
                    inner_loops: face
                        .inner_loops
                        .iter()
                        .map(|&lid| id_to_uuid(lid as u64))
                        .collect(),
                    orientation: matches!(face.orientation, FaceOrientation::Forward),
                },
            ));
        }

        // ── Shells ──
        for (shid, shell) in model.shells.iter() {
            let uuid = id_to_uuid(shid as u64);
            snapshot.shells.push((
                uuid,
                ShellData {
                    faces: shell
                        .faces
                        .iter()
                        .map(|&fid| id_to_uuid(fid as u64))
                        .collect(),
                    is_closed: matches!(shell.shell_type, GeoShellType::Closed),
                    shell_type: match shell.shell_type {
                        GeoShellType::Closed => ShellType::Closed,
                        GeoShellType::Open => ShellType::Open,
                        _ => ShellType::Open,
                    },
                },
            ));
        }

        // ── Solids ──
        for (sid, solid) in model.solids.iter() {
            let uuid = id_to_uuid(sid as u64);
            let mut shells = vec![id_to_uuid(solid.outer_shell as u64)];
            for &inner in &solid.inner_shells {
                shells.push(id_to_uuid(inner as u64));
            }
            snapshot.solids.push((
                uuid,
                SolidData {
                    shells,
                    feature_type: solid.name.clone(),
                },
            ));
        }

        snapshot
    }

    /// Convert from snapshot to BRepModel (import path)
    /// Reconstruct a [`BRepModel`] from this snapshot — the inverse of
    /// [`Self::from_model`].
    ///
    /// The snapshot keys every entity by a deterministic source UUID (see
    /// [`id_to_uuid`]); the kernel stores mint their own fresh ids on
    /// insertion, so we build a UUID→new-id map per entity type and walk
    /// the topology in dependency order
    /// (vertices → curves → surfaces → edges → loops → faces → shells →
    /// solids), translating each reference through the maps. References
    /// that fail to resolve (a malformed or partial snapshot) cause that
    /// entity to be skipped rather than panicking — import is best-effort
    /// and the [`Default`]-derived empty grids degrade gracefully.
    pub fn to_model(&self) -> BRepModel {
        use geometry_engine::primitives::r#loop::LoopType;
        use std::collections::HashMap;

        let mut model = BRepModel::new();

        let pt = |a: [f64; 3]| Point3::new(a[0], a[1], a[2]);
        let vec = |a: [f64; 3]| Vector3::new(a[0], a[1], a[2]);

        // ── Vertices ── (no dependencies)
        let mut vmap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, v) in &self.vertices {
            let id = model.vertices.add_unchecked_with_tolerance(
                v.position[0],
                v.position[1],
                v.position[2],
                v.tolerance,
            );
            vmap.insert(*uuid, id);
        }

        // ── Curves ──
        let mut cmap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, c) in &self.curves {
            if let Some(curve) = build_curve_from_data(c) {
                let id = model.curves.add(curve);
                cmap.insert(*uuid, id);
            }
        }

        // ── Surfaces ──
        let mut smap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, s) in &self.surfaces {
            let surface: Option<Box<dyn Surface>> = match s {
                SurfaceData::Plane { origin, normal } => {
                    GeoPlane::from_point_normal(pt(*origin), vec(*normal))
                        .ok()
                        .map(|p| Box::new(p) as Box<dyn Surface>)
                }
                SurfaceData::Cylinder {
                    origin,
                    axis,
                    radius,
                } => GeoCylinder::new(pt(*origin), vec(*axis), *radius)
                    .ok()
                    .map(|c| Box::new(c) as Box<dyn Surface>),
                SurfaceData::Sphere { center, radius } => GeoSphere::new(pt(*center), *radius)
                    .ok()
                    .map(|s| Box::new(s) as Box<dyn Surface>),
                SurfaceData::Cone {
                    apex,
                    axis,
                    half_angle,
                } => GeoCone::new(pt(*apex), vec(*axis), *half_angle)
                    .ok()
                    .map(|c| Box::new(c) as Box<dyn Surface>),
                SurfaceData::Torus {
                    center,
                    axis,
                    major_radius,
                    minor_radius,
                } => GeoTorus::new(pt(*center), vec(*axis), *major_radius, *minor_radius)
                    .ok()
                    .map(|t| Box::new(t) as Box<dyn Surface>),
                SurfaceData::BSpline {
                    control_points,
                    knots_u,
                    knots_v,
                    degree_u,
                    degree_v,
                } => {
                    let cps: Vec<Vec<Point3>> = control_points
                        .iter()
                        .map(|row| row.iter().map(|p| pt(*p)).collect())
                        .collect();
                    let weights: Vec<Vec<f64>> =
                        cps.iter().map(|row| vec![1.0; row.len()]).collect();
                    geometry_engine::math::nurbs::NurbsSurface::new(
                        cps,
                        weights,
                        knots_u.clone(),
                        knots_v.clone(),
                        *degree_u as usize,
                        *degree_v as usize,
                    )
                    .ok()
                    .map(|nurbs| Box::new(GeneralNurbsSurface { nurbs }) as Box<dyn Surface>)
                }
                SurfaceData::Nurbs {
                    control_points,
                    weights,
                    knots_u,
                    knots_v,
                    degree_u,
                    degree_v,
                } => {
                    let cps: Vec<Vec<Point3>> = control_points
                        .iter()
                        .map(|row| row.iter().map(|p| pt(*p)).collect())
                        .collect();
                    geometry_engine::math::nurbs::NurbsSurface::new(
                        cps,
                        weights.clone(),
                        knots_u.clone(),
                        knots_v.clone(),
                        *degree_u as usize,
                        *degree_v as usize,
                    )
                    .ok()
                    .map(|nurbs| Box::new(GeneralNurbsSurface { nurbs }) as Box<dyn Surface>)
                }
                SurfaceData::SurfaceOfRevolution {
                    axis_origin,
                    axis_direction,
                    profile,
                    angle,
                } => build_curve_from_data(profile).and_then(|pc| {
                    GeoSurfaceOfRevolution::new(pt(*axis_origin), vec(*axis_direction), pc, *angle)
                        .ok()
                        .map(|s| Box::new(s) as Box<dyn Surface>)
                }),
            };
            if let Some(surface) = surface {
                let id = model.surfaces.add(surface);
                smap.insert(*uuid, id);
            }
        }

        // ── Edges ── (depend on vertices + curves)
        use geometry_engine::primitives::curve::ParameterRange;
        let mut emap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, e) in &self.edges {
            let (Some(&start), Some(&end)) = (vmap.get(&e.start_vertex), vmap.get(&e.end_vertex))
            else {
                continue;
            };
            let curve_id = match e.curve.and_then(|c| cmap.get(&c).copied()) {
                Some(c) => c,
                None => continue,
            };
            let orientation = if e.orientation {
                EdgeOrientation::Forward
            } else {
                EdgeOrientation::Backward
            };
            let edge = Edge::new(0, start, end, curve_id, orientation, ParameterRange::unit());
            let id = model.edges.add(edge);
            emap.insert(*uuid, id);
        }

        // ── Loops ── (depend on edges)
        let mut lmap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, l) in &self.loops {
            let loop_type = if l.is_outer {
                LoopType::Outer
            } else {
                LoopType::Inner
            };
            let mut lp = Loop::with_capacity(0, loop_type, l.edges.len());
            for (i, edge_uuid) in l.edges.iter().enumerate() {
                if let Some(&eid) = emap.get(edge_uuid) {
                    let fwd = l.orientations.get(i).copied().unwrap_or(true);
                    lp.add_edge(eid, fwd);
                }
            }
            let id = model.loops.add(lp);
            lmap.insert(*uuid, id);
        }

        // ── Faces ── (depend on surfaces + loops)
        let mut fmap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, f) in &self.faces {
            let surface_id = match f.surface.and_then(|s| smap.get(&s).copied()) {
                Some(s) => s,
                None => continue,
            };
            let outer_loop = match f.outer_loop.and_then(|l| lmap.get(&l).copied()) {
                Some(l) => l,
                None => continue,
            };
            let orientation = if f.orientation {
                FaceOrientation::Forward
            } else {
                FaceOrientation::Backward
            };
            let mut face = Face::new(0, surface_id, outer_loop, orientation);
            for inner_uuid in &f.inner_loops {
                if let Some(&lid) = lmap.get(inner_uuid) {
                    face.add_inner_loop(lid);
                }
            }
            let id = model.faces.add(face);
            fmap.insert(*uuid, id);
        }

        // ── Shells ── (depend on faces)
        let mut shmap: HashMap<Uuid, u32> = HashMap::new();
        for (uuid, sh) in &self.shells {
            let shell_type = match sh.shell_type {
                ShellType::Closed => GeoShellType::Closed,
                ShellType::Open => GeoShellType::Open,
                ShellType::Compound => GeoShellType::Open,
            };
            let mut shell = Shell::new(0, shell_type);
            for face_uuid in &sh.faces {
                if let Some(&fid) = fmap.get(face_uuid) {
                    shell.add_face(fid);
                }
            }
            let id = model.shells.add(shell);
            shmap.insert(*uuid, id);
        }

        // ── Solids ── (depend on shells; shells[0] is the outer shell)
        for (_uuid, sd) in &self.solids {
            let mut shell_ids = sd.shells.iter().filter_map(|u| shmap.get(u).copied());
            let Some(outer) = shell_ids.next() else {
                continue;
            };
            let mut solid = Solid::new(0, outer);
            for inner in shell_ids {
                solid.add_inner_shell(inner);
            }
            solid.name = sd.feature_type.clone();
            model.solids.add(solid);
        }

        model
    }
}

impl Default for BRepSnapshot {
    fn default() -> Self {
        Self::new()
    }
}

// ── Helper functions for model extraction ──

/// Convert a u32/u64 topology ID to a deterministic UUID (namespace-based)
///
/// `pub(crate)` so the STEP pcurve builder can key its parameter-curve map by
/// the SAME deterministic UUIDs the snapshot uses for edges/faces, letting the
/// writer look up a pcurve by the snapshot edge id without re-deriving the
/// mapping.
pub(crate) fn id_to_uuid(id: u64) -> Uuid {
    // Use a fixed namespace to make IDs deterministic and reversible
    let bytes = id.to_le_bytes();
    let mut uuid_bytes = [0u8; 16];
    // Namespace prefix "ROSHERA\0" + 8 bytes of ID
    uuid_bytes[0..8].copy_from_slice(b"ROSHERA\0");
    uuid_bytes[8..16].copy_from_slice(&bytes);
    Uuid::from_bytes(uuid_bytes)
}

/// Extract curve parameters into serializable CurveData
/// Reconstruct a kernel curve from its serialized `CurveData`. Shared by the
/// curve-import loop and the surface-of-revolution profile reconstruction.
fn build_curve_from_data(c: &CurveData) -> Option<Box<dyn Curve>> {
    let pt = |a: [f64; 3]| Point3::new(a[0], a[1], a[2]);
    let vc = |a: [f64; 3]| Vector3::new(a[0], a[1], a[2]);
    match c {
        CurveData::Line { start, end } => Some(Box::new(GeoLine::new(pt(*start), pt(*end)))),
        CurveData::Circle {
            center,
            normal,
            radius,
        } => GeoCircle::new(pt(*center), vc(*normal), *radius)
            .ok()
            .map(|c| Box::new(c) as Box<dyn Curve>),
        CurveData::Arc {
            center,
            normal,
            radius,
            start_angle,
            end_angle,
        } => GeoArc::new(
            pt(*center),
            vc(*normal),
            *radius,
            *start_angle,
            *end_angle - *start_angle,
        )
        .ok()
        .map(|a| Box::new(a) as Box<dyn Curve>),
        CurveData::BSpline {
            control_points,
            knots,
            degree,
        } => {
            let cps: Vec<Point3> = control_points.iter().map(|p| pt(*p)).collect();
            let weights = vec![1.0; cps.len()];
            GeoNurbsCurve::new(*degree as usize, cps, weights, knots.clone())
                .ok()
                .map(|n| Box::new(n) as Box<dyn Curve>)
        }
        CurveData::Nurbs {
            control_points,
            weights,
            knots,
            degree,
        } => {
            let cps: Vec<Point3> = control_points.iter().map(|p| pt(*p)).collect();
            GeoNurbsCurve::new(*degree as usize, cps, weights.clone(), knots.clone())
                .ok()
                .map(|n| Box::new(n) as Box<dyn Curve>)
        }
    }
}

fn extract_curve_data(curve: &dyn Curve) -> CurveData {
    use geometry_engine::primitives::curve::{Arc, Circle, Line, NurbsCurve};

    let any = curve.as_any();

    if let Some(line) = any.downcast_ref::<Line>() {
        return CurveData::Line {
            start: [line.start.x, line.start.y, line.start.z],
            end: [line.end.x, line.end.y, line.end.z],
        };
    }

    if let Some(circle) = any.downcast_ref::<Circle>() {
        let center = circle.center();
        let normal = circle.normal();
        let radius = circle.radius();
        return CurveData::Circle {
            center: [center.x, center.y, center.z],
            normal: [normal.x, normal.y, normal.z],
            radius,
        };
    }

    if let Some(arc) = any.downcast_ref::<Arc>() {
        // The STEP writer emits an arc as its *unbounded* basis CIRCLE; the
        // importer re-derives the trimmed extent as the CCW sweep from the edge's
        // start vertex to its end vertex ABOUT THE EMITTED NORMAL. That guess is
        // only correct when CCW(eval0 → eval1) about the stored normal follows the
        // arc's actual direction — but `arc.normal` can point the other way for a
        // fillet-band end-arc whose sweep the kernel stores CW about it, so a 90°
        // arc re-imports as its 270° complement (#42). Re-derive the emitted
        // normal straight from the geometry: `(P0−C) × (Pmid−C)` is the axis about
        // which the first half of the arc turns CCW, i.e. the orientation for
        // which CCW(P0 → P1) follows through the midpoint = the actual arc. This
        // is self-correcting regardless of how the kernel happened to store the
        // normal, and leaves already-correctly-oriented arcs unchanged. (The
        // importer additionally negates this for `same_sense = .F.` edges, so both
        // traversal senses reconstruct the same geometric arc.)
        let c = arc.center;
        let emit_normal = match (arc.evaluate(0.0), arc.evaluate(0.5)) {
            (Ok(p0), Ok(pm)) => {
                let v0 = p0.position - c;
                let vm = pm.position - c;
                v0.cross(&vm).normalize().unwrap_or_else(|_| arc.normal)
            }
            _ => arc.normal,
        };
        return CurveData::Arc {
            center: [c.x, c.y, c.z],
            normal: [emit_normal.x, emit_normal.y, emit_normal.z],
            radius: arc.radius,
            start_angle: arc.start_angle,
            end_angle: arc.start_angle + arc.sweep_angle,
        };
    }

    if let Some(nurbs) = any.downcast_ref::<NurbsCurve>() {
        let cps: Vec<[f64; 3]> = nurbs
            .control_points
            .iter()
            .map(|p| [p.x, p.y, p.z])
            .collect();
        if nurbs.weights.iter().all(|&w| (w - 1.0).abs() < 1e-12) {
            // Non-rational — store as BSpline
            return CurveData::BSpline {
                control_points: cps,
                knots: nurbs.knots.clone(),
                degree: nurbs.degree as u32,
            };
        }
        return CurveData::Nurbs {
            control_points: cps,
            weights: nurbs.weights.clone(),
            knots: nurbs.knots.clone(),
            degree: nurbs.degree as u32,
        };
    }

    // Fallback: sample the curve as a polyline and store as a degree-1
    // B-spline. The knot vector MUST be a valid clamped vector sized to
    // the control-point count (`n + degree + 1`), NOT empty — an empty
    // knot vector serializes to `B_SPLINE_CURVE_WITH_KNOTS(…,(),())`,
    // which every conforming reader (Roshera's importer, OCCT/FreeCAD)
    // rejects, dropping the edge and tearing a topology gap. A degree-1
    // clamped uniform vector reproduces the sampled polyline exactly.
    let n_samples = 20;
    let mut cps = Vec::with_capacity(n_samples + 1);
    for i in 0..=n_samples {
        let t = i as f64 / n_samples as f64;
        if let Ok(pt) = curve.point_at(t) {
            cps.push([pt.x, pt.y, pt.z]);
        }
    }
    let degree = 1;
    let knots = clamped_uniform_knots(cps.len(), degree);
    CurveData::BSpline {
        control_points: cps,
        knots,
        degree: degree as u32,
    }
}

/// Build a clamped, uniformly-spaced knot vector for a B-spline with
/// `n` control points and the given `degree`.
///
/// The returned vector has the schema-mandated length `n + degree + 1`:
/// the first and last knots each carry multiplicity `degree + 1`
/// (clamping the curve/surface to its end control points), and the
/// interior knots step uniformly `1, 2, …, n - degree - 1`. The
/// parameter domain is therefore `[0, n - degree]`, which is
/// non-degenerate whenever `n > degree` (the caller guarantees this by
/// sampling more points than the degree).
///
/// This is the inverse of the importer's `expand_knot_vector`: the
/// writer collapses this expanded vector back into `(distinct, mult)`
/// pairs, the importer re-expands it, and the two agree exactly.
fn clamped_uniform_knots(n: usize, degree: usize) -> Vec<f64> {
    // Guard: a valid clamped vector needs n > degree. If the sampler
    // produced too few points, fall back to the minimum legal grid by
    // clamping the interior span count to zero (Bézier-like), which
    // still yields a non-degenerate domain of length 1.
    let interior = n.saturating_sub(degree + 1);
    let span_max = (interior + 1) as f64; // domain end = n - degree
    let mut knots = Vec::with_capacity(n + degree + 1);
    for _ in 0..=degree {
        knots.push(0.0);
    }
    for i in 1..=interior {
        knots.push(i as f64);
    }
    for _ in 0..=degree {
        knots.push(span_max);
    }
    knots
}

/// Extract surface parameters into serializable SurfaceData
/// If `fillet`'s spine is a straight line, return `(origin, axis)` for the
/// exact cylinder it lies on (origin = spine start, a point on the axis; axis =
/// unit spine direction). Returns `None` for a curved spine (a canal surface,
/// not a cylinder). Straightness is decided geometrically — every interior
/// spine sample must lie on the chord through the endpoints — so it is
/// independent of which concrete `Curve` type backs the spine.
fn straight_fillet_cylinder_axis(fillet: &CylindricalFillet) -> Option<([f64; 3], [f64; 3])> {
    let p0 = fillet.spine.evaluate(0.0).ok()?.position;
    let p1 = fillet.spine.evaluate(1.0).ok()?.position;
    let ax = [p1.x - p0.x, p1.y - p0.y, p1.z - p0.z];
    let len = (ax[0] * ax[0] + ax[1] * ax[1] + ax[2] * ax[2]).sqrt();
    if len < 1e-9 {
        return None; // degenerate spine
    }
    let axis = [ax[0] / len, ax[1] / len, ax[2] / len];
    // Scale-relative straightness tolerance: interior samples' perpendicular
    // offset from the chord must vanish.
    let tol = 1e-6 * len.max(1.0);
    for k in 1..8 {
        let t = k as f64 / 8.0;
        let p = fillet.spine.evaluate(t).ok()?.position;
        let d = [p.x - p0.x, p.y - p0.y, p.z - p0.z];
        let along = d[0] * axis[0] + d[1] * axis[1] + d[2] * axis[2];
        let perp = [
            d[0] - axis[0] * along,
            d[1] - axis[1] * along,
            d[2] - axis[2] * along,
        ];
        let perp_mag = (perp[0] * perp[0] + perp[1] * perp[1] + perp[2] * perp[2]).sqrt();
        if perp_mag > tol {
            return None; // curved spine → canal surface, not a cylinder
        }
    }
    Some(([p0.x, p0.y, p0.z], axis))
}

/// If `ruled` is an EXACT translational sweep of a NURBS rail, return the
/// equivalent exact `SurfaceData::BSpline` / `SurfaceData::Nurbs`.
///
/// Preconditions verified (any failure → `None`, caller falls back to the
/// sampled grid):
///
/// 1. **Shared basis** — both rails' `to_nurbs()` forms agree in degree,
///    knot vector and weights (a ruled surface between rails with
///    different bases is not a tensor-product surface on either basis).
/// 2. **Knot domain [0, 1]** — `RuledSurface` feeds its `u ∈ [0, 1]`
///    RAW to the rails, and a STEP reader evaluates the written surface
///    on its knot domain; the two parameterisations coincide only when
///    the knot vector spans exactly [0, 1] (which the extrude path
///    guarantees: profile-edge NURBS and `subcurve` halves are
///    [0, 1]-normalised). Emitting a different domain would silently
///    invalidate every exported pcurve on the face.
/// 3. **Constant displacement** — every top CP is the matching bottom CP
///    plus one shared vector `d` (the extrusion sweep).
/// 4. **Parameterisation identity** — the rails evaluate identically to
///    their NURBS forms at sampled parameters. Exact (0-distance) for
///    true `NurbsCurve` rails; REJECTS rails whose `to_nurbs()`
///    re-parameterises (an `Arc`'s rational-Bézier form is angle-
///    nonlinear), because the written surface would disagree with the
///    live surface's (u, v) frame and distort every projected pcurve.
///
/// With 1–4 satisfied, `S(u,v) = (1−v)·C_b(u) + v·C_t(u)` equals the
/// degree-(p, 1) NURBS surface with rows `[P_b_i, P_b_i + d]` and per-row
/// duplicated weights: the v-linear combination happens in homogeneous
/// space with identical row weights, so the rational denominator is
/// v-independent and the surface is pointwise identical, not fitted.
fn exact_swept_ruled_surface(ruled: &RuledSurface) -> Option<SurfaceData> {
    let b = ruled.curve1.to_nurbs();
    let t = ruled.curve2.to_nurbs();

    // 1. Shared basis.
    if b.degree != t.degree
        || b.control_points.len() != t.control_points.len()
        || b.knots.len() != t.knots.len()
    {
        return None;
    }
    if !b
        .knots
        .iter()
        .zip(t.knots.iter())
        .all(|(x, y)| (x - y).abs() < 1e-12)
    {
        return None;
    }
    if !b
        .weights
        .iter()
        .zip(t.weights.iter())
        .all(|(x, y)| (x - y).abs() < 1e-12)
    {
        return None;
    }

    // 2. Knot domain [0, 1].
    let (first, last) = (b.knots.first()?, b.knots.last()?);
    if first.abs() > 1e-12 || (last - 1.0).abs() > 1e-12 {
        return None;
    }

    // 3. Constant displacement between the control nets.
    let p0b = b.control_points.first()?;
    let p0t = t.control_points.first()?;
    let d = *p0t - *p0b;
    let scale = b
        .control_points
        .iter()
        .chain(t.control_points.iter())
        .map(|p| p.x.abs().max(p.y.abs()).max(p.z.abs()))
        .fold(1.0_f64, f64::max);
    let tol = 1e-9 * scale;
    if !b
        .control_points
        .iter()
        .zip(t.control_points.iter())
        .all(|(pb, pt)| (*pt - (*pb + d)).magnitude() <= tol)
    {
        return None;
    }

    // 4. Parameterisation identity of each rail with its NURBS form.
    for u in [0.0, 0.17, 0.5, 0.83, 1.0] {
        let rb = ruled.curve1.point_at(u).ok()?;
        let nb = b.point_at(u).ok()?;
        if rb.distance(&nb) > tol {
            return None;
        }
        let rt = ruled.curve2.point_at(u).ok()?;
        let nt = t.point_at(u).ok()?;
        if rt.distance(&nt) > tol {
            return None;
        }
    }

    // Control net: rows in [u][v] order, v-degree 1 over knots
    // [0, 0, 1, 1] (domain [0, 1] — matches the ruled v exactly). The
    // top row uses the STORED top CPs (exact), not `pb + d`.
    let control_points: Vec<Vec<[f64; 3]>> = b
        .control_points
        .iter()
        .zip(t.control_points.iter())
        .map(|(pb, pt)| vec![[pb.x, pb.y, pb.z], [pt.x, pt.y, pt.z]])
        .collect();
    let knots_u = b.knots.clone();
    let knots_v = vec![0.0, 0.0, 1.0, 1.0];
    let degree_u = b.degree as u32;
    let degree_v = 1u32;

    let rational = b.weights.iter().any(|w| (w - 1.0).abs() > 1e-12);
    if rational {
        let weights: Vec<Vec<f64>> = b.weights.iter().map(|&w| vec![w, w]).collect();
        Some(SurfaceData::Nurbs {
            control_points,
            weights,
            knots_u,
            knots_v,
            degree_u,
            degree_v,
        })
    } else {
        Some(SurfaceData::BSpline {
            control_points,
            knots_u,
            knots_v,
            degree_u,
            degree_v,
        })
    }
}

fn extract_surface_data(surface: &dyn Surface) -> SurfaceData {
    let any = surface.as_any();

    if let Some(plane) = any.downcast_ref::<GeoPlane>() {
        return SurfaceData::Plane {
            origin: [plane.origin.x, plane.origin.y, plane.origin.z],
            normal: [plane.normal.x, plane.normal.y, plane.normal.z],
        };
    }

    if let Some(cyl) = any.downcast_ref::<GeoCylinder>() {
        return SurfaceData::Cylinder {
            origin: [cyl.origin.x, cyl.origin.y, cyl.origin.z],
            axis: [cyl.axis.x, cyl.axis.y, cyl.axis.z],
            radius: cyl.radius,
        };
    }

    // A fillet on a STRAIGHT edge (e.g. a box edge) is an exact circular
    // CYLINDER: the rolling ball of constant `radius` rolls along a straight
    // spine (the cylinder centre line), sweeping a quarter-band of a genuine
    // cylinder. The kernel keeps it as a distinct `CylindricalFillet` (its own
    // trim frame), which the analytic downcasts above miss — so without this
    // branch it fell to the degree-1 B-spline grid fallback below and STEP
    // emitted `B_SPLINE_SURFACE` instead of `CYLINDRICAL_SURFACE`. The band then
    // re-imported as a NURBS patch that tessellates OPEN (its sampled boundary
    // has no twin on the adjacent planar caps → T-junctions; #42). Emit the
    // exact `CYLINDRICAL_SURFACE` here, mirroring how a bore-rim `Torus` fillet
    // emits `TOROIDAL_SURFACE`; the quarter extent lives in the trim loop, not
    // the surface, exactly as ISO 10303-42 intends.
    //
    // TIGHTLY GATED to a straight spine: a fillet on a CURVED edge is a
    // canal/pipe surface, NOT a cylinder — it must keep falling through to the
    // sampled B-spline path, so it is not misdeclared as an infinite cylinder.
    if let Some(fillet) = any.downcast_ref::<CylindricalFillet>() {
        if let Some((origin, axis)) = straight_fillet_cylinder_axis(fillet) {
            return SurfaceData::Cylinder {
                origin,
                axis,
                radius: fillet.radius,
            };
        }
    }

    if let Some(sph) = any.downcast_ref::<GeoSphere>() {
        return SurfaceData::Sphere {
            center: [sph.center.x, sph.center.y, sph.center.z],
            radius: sph.radius,
        };
    }

    if let Some(cone) = any.downcast_ref::<GeoCone>() {
        return SurfaceData::Cone {
            apex: [cone.apex.x, cone.apex.y, cone.apex.z],
            axis: [cone.axis.x, cone.axis.y, cone.axis.z],
            half_angle: cone.half_angle,
        };
    }

    if let Some(torus) = any.downcast_ref::<GeoTorus>() {
        return SurfaceData::Torus {
            center: [torus.center.x, torus.center.y, torus.center.z],
            axis: [torus.axis.x, torus.axis.y, torus.axis.z],
            major_radius: torus.major_radius,
            minor_radius: torus.minor_radius,
        };
    }

    if let Some(nurbs_surf) = any.downcast_ref::<GeneralNurbsSurface>() {
        let cps: Vec<Vec<[f64; 3]>> = nurbs_surf
            .nurbs
            .control_points
            .iter()
            .map(|row| row.iter().map(|p| [p.x, p.y, p.z]).collect())
            .collect();
        let weights: Vec<Vec<f64>> = nurbs_surf.nurbs.weights.clone();
        let all_unit = weights
            .iter()
            .all(|row| row.iter().all(|&w| (w - 1.0).abs() < 1e-12));

        if all_unit {
            return SurfaceData::BSpline {
                control_points: cps,
                knots_u: nurbs_surf.nurbs.knots_u.values().to_vec(),
                knots_v: nurbs_surf.nurbs.knots_v.values().to_vec(),
                degree_u: nurbs_surf.nurbs.degree_u as u32,
                degree_v: nurbs_surf.nurbs.degree_v as u32,
            };
        }
        return SurfaceData::Nurbs {
            control_points: cps,
            weights,
            knots_u: nurbs_surf.nurbs.knots_u.values().to_vec(),
            knots_v: nurbs_surf.nurbs.knots_v.values().to_vec(),
            degree_u: nurbs_surf.nurbs.degree_u as u32,
            degree_v: nurbs_surf.nurbs.degree_v as u32,
        };
    }

    // An EXACTLY-SWEPT ruled lateral (SKETCH-DCM #45 follow-ups C item 2):
    // both rails share one NURBS basis (same degree / knots / weights) and
    // the top rail is the bottom rail translated by a constant vector —
    // the extrude walls of NURBS/ellipse/oblique-spline profiles. Such a
    // surface IS a NURBS surface: control net = rail CPs swept along the
    // displacement (v-degree 1), weights preserved per row. Emitting it
    // exactly maps the wall to a proper `B_SPLINE_SURFACE_WITH_KNOTS`
    // (rational complex form for rational rails) instead of the degree-1
    // sampled grid below. Non-swept / re-parameterised ruled surfaces
    // fall through to the sampled fallback unchanged.
    if let Some(ruled) = any.downcast_ref::<RuledSurface>() {
        if let Some(exact) = exact_swept_ruled_surface(ruled) {
            return exact;
        }
    }

    if let Some(sor) = any.downcast_ref::<GeoSurfaceOfRevolution>() {
        // Exact: a profile curve revolved about an axis → STEP SURFACE_OF_REVOLUTION.
        // The profile reuses the analytic curve paths (Line/Circle/Arc/NURBS), so a
        // revolved nozzle/vessel exports smooth instead of the degree-1 grid below
        // (the FreeCAD faceted-nozzle bug).
        return SurfaceData::SurfaceOfRevolution {
            axis_origin: [sor.axis_origin.x, sor.axis_origin.y, sor.axis_origin.z],
            axis_direction: [
                sor.axis_direction.x,
                sor.axis_direction.y,
                sor.axis_direction.z,
            ],
            profile: extract_curve_data(&*sor.profile_curve),
            angle: sor.angle,
        };
    }

    // Fallback: sample the surface on a grid and store as a degree-1
    // B-spline. As with the curve fallback, the knot vectors MUST be
    // valid clamped vectors sized to the control-point grid
    // (`n + degree + 1` per direction), NOT empty — an empty knot
    // vector serializes to `B_SPLINE_SURFACE_WITH_KNOTS(…,(),(),(),())`,
    // which the importer rejects ("knot vector is empty"), dropping the
    // face and tearing topology gaps in every adjacent edge. This was
    // the root cause of Roshera-exported solids failing to re-import as
    // watertight: boolean-split faces whose surface type the analytic
    // downcasts above didn't recognise fell here and lost their knots.
    let n = 10;
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    let mut cps: Vec<Vec<[f64; 3]>> = Vec::with_capacity(n + 1);
    for i in 0..=n {
        let u = u_min + (u_max - u_min) * i as f64 / n as f64;
        let mut row = Vec::with_capacity(n + 1);
        for j in 0..=n {
            let v = v_min + (v_max - v_min) * j as f64 / n as f64;
            if let Ok(pt) = surface.point_at(u, v) {
                row.push([pt.x, pt.y, pt.z]);
            } else {
                row.push([0.0, 0.0, 0.0]);
            }
        }
        cps.push(row);
    }
    let (degree_u, degree_v) = (1usize, 1usize);
    let knots_u = clamped_uniform_knots(cps.len(), degree_u);
    let knots_v = clamped_uniform_knots(cps.first().map(|r| r.len()).unwrap_or(0), degree_v);
    SurfaceData::BSpline {
        control_points: cps,
        knots_u,
        knots_v,
        degree_u: degree_u as u32,
        degree_v: degree_v as u32,
    }
}

#[cfg(test)]
mod knot_tests {
    use super::clamped_uniform_knots;

    /// The clamped vector has the schema-mandated length `n + degree + 1`,
    /// clamps both ends to multiplicity `degree + 1`, and spans a
    /// non-degenerate, strictly-increasing interior — so the kernel's
    /// `KnotVector::validate(degree, n)` accepts it (the inverse of the
    /// "knot vector is empty" import failure this fixes).
    #[test]
    fn clamped_uniform_knots_are_valid_for_degree_one() {
        use geometry_engine::math::bspline::KnotVector;
        for n in 2..=20usize {
            let degree = 1usize;
            let k = clamped_uniform_knots(n, degree);
            assert_eq!(k.len(), n + degree + 1, "knot count n={n}");
            // Clamped ends.
            assert_eq!(k[0], k[degree], "start clamp n={n}");
            let last = k.len() - 1;
            assert_eq!(k[last], k[last - degree], "end clamp n={n}");
            // Non-decreasing.
            assert!(k.windows(2).all(|w| w[1] >= w[0]), "monotone n={n}");
            // Non-degenerate domain.
            assert!(k[last] > k[0], "domain non-degenerate n={n}");
            // The kernel accepts it.
            let kv = KnotVector::new(k).expect("knot vector constructs");
            kv.validate(degree, n)
                .unwrap_or_else(|e| panic!("kernel rejected clamped knots n={n}: {e:?}"));
        }
    }

    #[test]
    fn clamped_uniform_knots_never_empty() {
        // Even a degenerate-small grid must not produce an empty vector
        // (that was the exact serialization that broke re-import).
        for n in 2..=4usize {
            assert!(!clamped_uniform_knots(n, 1).is_empty());
        }
    }
}

#[cfg(test)]
mod surface_of_revolution_export_tests {
    use super::{extract_surface_data, CurveData, SurfaceData};
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::primitives::curve::Line as GeoLine;
    use geometry_engine::primitives::surface::SurfaceOfRevolution as SoR;

    /// A SurfaceOfRevolution must export as an EXACT analytic surface, not the
    /// degree-1 grid fallback that faceted revolved parts in FreeCAD. The extract
    /// path must keep it analytic and preserve the profile curve exactly.
    #[test]
    fn surface_of_revolution_extracts_analytic_not_faceted() {
        // A vertical line at radius 5, revolved 2π about Z (a cylinder as a SoR).
        let profile = Box::new(GeoLine::new(
            Point3::new(5.0, 0.0, 0.0),
            Point3::new(5.0, 0.0, 10.0),
        ));
        let sor = SoR::new(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            profile,
            std::f64::consts::TAU,
        )
        .expect("surface of revolution constructs");

        match extract_surface_data(&sor) {
            SurfaceData::SurfaceOfRevolution {
                angle,
                profile,
                axis_direction,
                ..
            } => {
                assert!(
                    (angle - std::f64::consts::TAU).abs() < 1e-9,
                    "full-revolution angle preserved"
                );
                assert!(
                    matches!(profile, CurveData::Line { .. }),
                    "profile stays an exact Line, not a faceted polyline"
                );
                assert!((axis_direction[2] - 1.0).abs() < 1e-9, "axis is +Z");
            }
            other => panic!("SurfaceOfRevolution faceted to the fallback: {other:?}"),
        }
    }
}
