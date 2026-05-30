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
    r#loop::{Loop, LoopType},
    shell::{Shell, ShellType as GeoShellType},
    solid::Solid,
    surface::{
        Cone as GeoCone, Cylinder as GeoCylinder, GeneralNurbsSurface, Plane as GeoPlane,
        Sphere as GeoSphere, Surface, Torus as GeoTorus,
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
            let curve: Option<Box<dyn Curve>> = match c {
                CurveData::Line { start, end } => {
                    Some(Box::new(GeoLine::new(pt(*start), pt(*end))))
                }
                CurveData::Circle {
                    center,
                    normal,
                    radius,
                } => GeoCircle::new(pt(*center), vec(*normal), *radius)
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
                    vec(*normal),
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
            };
            if let Some(curve) = curve {
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
fn id_to_uuid(id: u64) -> Uuid {
    // Use a fixed namespace to make IDs deterministic and reversible
    let bytes = id.to_le_bytes();
    let mut uuid_bytes = [0u8; 16];
    // Namespace prefix "ROSHERA\0" + 8 bytes of ID
    uuid_bytes[0..8].copy_from_slice(b"ROSHERA\0");
    uuid_bytes[8..16].copy_from_slice(&bytes);
    Uuid::from_bytes(uuid_bytes)
}

/// Extract curve parameters into serializable CurveData
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
        return CurveData::Arc {
            center: [arc.center.x, arc.center.y, arc.center.z],
            normal: [arc.normal.x, arc.normal.y, arc.normal.z],
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

    // Fallback: sample the curve as a polyline and store as BSpline degree 1
    let n_samples = 20;
    let mut cps = Vec::with_capacity(n_samples + 1);
    for i in 0..=n_samples {
        let t = i as f64 / n_samples as f64;
        if let Ok(pt) = curve.point_at(t) {
            cps.push([pt.x, pt.y, pt.z]);
        }
    }
    CurveData::BSpline {
        control_points: cps,
        knots: Vec::new(), // Empty knots = sampled polyline
        degree: 1,
    }
}

/// Extract surface parameters into serializable SurfaceData
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

    // Fallback: sample the surface and store as BSpline approximation
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
    SurfaceData::BSpline {
        control_points: cps,
        knots_u: Vec::new(),
        knots_v: Vec::new(),
        degree_u: 1,
        degree_v: 1,
    }
}
