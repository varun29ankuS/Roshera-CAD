//! The ATTRIBUTED ADJACENCY GRAPH of a solid (Joshi & Chang 1988).
//!
//! Feature recognition on a body with no feature history ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â a STEP import ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â
//! starts from one structure: faces as nodes, shared edges as arcs, every arc
//! labelled by how its two faces meet. Joshi & Chang's AAG labels an arc
//! convex or concave; Gao & Shah's extension adds the surface attributes a
//! machined-feature rule needs (a hole is a CONCAVE cylinder; a counterbore
//! floor is a plane perpendicular to that cylinder's axis, meeting the larger
//! wall concavely and the smaller wall convexly). This module builds exactly
//! that, for one solid, deterministically:
//!
//! * **Nodes** ([`AagNode`]) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â one per face of the solid's shells, in face-id
//!   order, carrying the analytic surface ([`FaceGeometry`]: plane normal,
//!   cylinder axis + radius, cone apex + half-angle, ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦), the material side of
//!   a surface of revolution ([`AagNode::cavity`]: does the material surround
//!   the axis, as on a bore wall, or fill it, as on a boss), and the face's
//!   station range along that axis read off its trim EDGES (the surface's own
//!   `height_limits` is the uncut bound and goes stale after a boolean).
//! * **Arcs** ([`AagArc`]) ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â one per edge used by two face sides, in edge-id
//!   order, labelled [`EdgeConvexity::Convex`] / `Concave` / `Tangent` /
//!   `Seam`, with the [`ConvexityDerivation`] that produced the label.
//!
//! # Exact where the geometry is analytic
//!
//! Convexity is the sign of `n_a Ãƒâ€šÃ‚Â· t_b`, where `n_a` is face A's outward
//! normal at the edge and `t_b` is the direction that leaves the edge INTO
//! face B's material (a box edge: top normal `+z`, the side face runs `-z`
//! from the edge, product `-1`, convex). When face B is a cylinder or cone
//! and the edge is one of its circular rims, `t_b` is known in closed form ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â
//! the axis (cylinder) or the generator (cone), signed toward the face's own
//! station range ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â and both normals are analytic, so the label is
//! [`ConvexityDerivation::Analytic`]: no probe, no projection, no tolerance
//! band beyond the tangency test. Every other edge (planeÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Å“plane, freeform)
//! falls back to the kernel's one dihedral classifier
//! ([`crate::operations::edge_classification`]), labelled
//! [`ConvexityDerivation::Dihedral`]; an edge that classifier cannot sign is
//! [`EdgeConvexity::Undetermined`], never guessed.
//!
//! # Scope
//!
//! Incidence is built from the TARGET solid's own shells and loops, not from
//! a model-wide edge scan, so an orphaned shell another boolean left in the
//! store can never make a manifold edge read non-manifold here.

use crate::math::{Point3, Tolerance, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Torus};
use crate::primitives::topology_builder::BRepModel;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Relative tolerance for "the same" length (scaled by the feature's own
/// size, floored at 1 mm).
const REL_TOL: f64 = 1e-6;
/// Two unit normals are parallel (tangent faces) when `|n_a ÃƒÆ’Ã¢â‚¬â€ n_b|` is below
/// this ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â the same angular band the dihedral classifier uses.
const TANGENT_SIN_TOL: f64 = 1e-6;
/// Samples per edge for station / radius reads (endpoints included).
const EDGE_SAMPLES: usize = 8;

/// The analytic surface a face lies on, in world coordinates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum FaceGeometry {
    /// A plane. `normal` is the face's OUTWARD normal (orientation applied).
    Plane { origin: [f64; 3], normal: [f64; 3] },
    /// A circular cylinder. `axis` is the surface's unit axis as stored.
    Cylinder {
        origin: [f64; 3],
        axis: [f64; 3],
        radius: f64,
    },
    /// A circular cone. `half_angle` in radians.
    Cone {
        apex: [f64; 3],
        axis: [f64; 3],
        half_angle: f64,
    },
    /// A sphere.
    Sphere { center: [f64; 3], radius: f64 },
    /// A torus.
    Torus {
        center: [f64; 3],
        axis: [f64; 3],
        major_radius: f64,
        minor_radius: f64,
    },
    /// Any other surface (NURBS, blend, offset ÃƒÂ¢Ã¢â€šÂ¬Ã‚Â¦), by its type name. No
    /// analytic attribute is claimed for it.
    Other { type_name: String },
}

impl FaceGeometry {
    /// Axis line `(point, unit direction)` of a surface of revolution
    /// (cylinder, cone, torus); `None` for planes, spheres and freeform.
    pub fn axis_line(&self) -> Option<([f64; 3], [f64; 3])> {
        match self {
            FaceGeometry::Cylinder { origin, axis, .. } => Some((*origin, *axis)),
            FaceGeometry::Cone { apex, axis, .. } => Some((*apex, *axis)),
            FaceGeometry::Torus { center, axis, .. } => Some((*center, *axis)),
            _ => None,
        }
    }
}

/// One face of the solid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AagNode {
    /// The face id.
    pub face: u32,
    /// Its analytic surface.
    pub geometry: FaceGeometry,
    /// For a cylinder or cone: `Some(true)` when the material SURROUNDS the
    /// axis (outward normal points toward it ÃƒÂ¢Ã¢â€šÂ¬Ã¢â‚¬Â a bore or countersink wall),
    /// `Some(false)` when it fills the axis (a boss, the part's OD). `None`
    /// for every other surface.
    pub cavity: Option<bool>,
    /// For a cylinder or cone: the face's `[min, max]` station along the
    /// surface axis (measured from the axis point of
    /// [`FaceGeometry::axis_line`]), read off its trim edges.
    pub station_range: Option<[f64; 2]>,
    /// Number of loops (outer + inner) bounding the face.
    pub loop_count: usize,
    /// Indices into [`AttributedAdjacencyGraph::arcs`] of every arc this face
    /// takes part in, ascending.
    pub arcs: Vec<usize>,
}

/// How the two faces of an edge meet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeConvexity {
    /// The material wedge at the edge is less than 180Ãƒâ€šÃ‚Â° (a box edge, a hole's
    /// opening rim).
    Convex,
    /// The material wedge exceeds 180Ãƒâ€šÃ‚Â° (a boss's root, a counterbore's floor
    /// corner, a blind hole's bottom).
    Concave,
    /// The two faces share a tangent plane along the edge (G1).
    Tangent,
    /// The edge is a seam: one face meets itself across it.
    Seam,
    /// No sign could be derived (a degenerate edge, a failed classification).
    Undetermined,
}

/// Which computation produced an [`EdgeConvexity`] label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConvexityDerivation {
    /// Closed-form from analytic normals and a circular rim of a cylinder or
    /// cone (see the module docs).
    Analytic,
    /// The kernel's dihedral classifier
    /// ([`crate::operations::edge_classification`]).
    Dihedral,
    /// Structural (a seam) or no derivation succeeded.
    Structural,
}

/// A circular rim: the edge is a circle (or an arc of one) about an axis.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RimCircle {
    /// Station of the circle's plane along the axis of the face it was read
    /// against.
    pub station: f64,
    /// Radius of the circle about that axis.
    pub radius: f64,
}

/// One edge shared by two face sides.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AagArc {
    /// The edge id.
    pub edge: u32,
    /// The two faces, ascending (equal for a seam).
    pub faces: [u32; 2],
    /// How they meet.
    pub convexity: EdgeConvexity,
    /// What produced [`Self::convexity`].
    pub derivation: ConvexityDerivation,
    /// True when both faces lie on the same analytic surface (a face the
    /// boolean split in two): a consumer reading FEATURES merges across it.
    pub same_surface: bool,
}

/// The attributed adjacency graph of one solid.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttributedAdjacencyGraph {
    /// The solid it describes.
    pub solid: SolidId,
    /// Every face, ascending by face id.
    pub nodes: Vec<AagNode>,
    /// Every two-sided edge, ascending by edge id.
    pub arcs: Vec<AagArc>,
    /// Edges used by exactly one face side (an open shell's border).
    pub boundary_edges: Vec<u32>,
    /// Edges used by three or more face sides.
    pub non_manifold_edges: Vec<u32>,
}

impl AttributedAdjacencyGraph {
    /// The node for `face`, if the face belongs to the solid.
    pub fn node(&self, face: u32) -> Option<&AagNode> {
        self.nodes
            .binary_search_by_key(&face, |n| n.face)
            .ok()
            .map(|i| &self.nodes[i])
    }

    /// Every `(arc, other face)` pair incident to `face`, in arc order. A seam
    /// arc reports the face itself as the other side.
    pub fn neighbours(&self, face: u32) -> Vec<(&AagArc, u32)> {
        let Some(n) = self.node(face) else {
            return Vec::new();
        };
        n.arcs
            .iter()
            .map(|&i| {
                let a = &self.arcs[i];
                let other = if a.faces[0] == face {
                    a.faces[1]
                } else {
                    a.faces[0]
                };
                (a, other)
            })
            .collect()
    }

    /// The arcs shared by faces `a` and `b`, in edge order.
    pub fn arcs_between(&self, a: u32, b: u32) -> Vec<&AagArc> {
        let key = if a <= b { [a, b] } else { [b, a] };
        self.arcs.iter().filter(|x| x.faces == key).collect()
    }
}

fn v3(p: Vector3) -> [f64; 3] {
    [p.x, p.y, p.z]
}
fn p3(p: Point3) -> [f64; 3] {
    [p.x, p.y, p.z]
}
fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm(a: [f64; 3]) -> f64 {
    dot(a, a).sqrt()
}
fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let n = norm(a);
    (n > 1e-12).then(|| scale(a, 1.0 / n))
}

/// Station of `p` along the axis `(o, k)` and its radial distance from it.
pub(crate) fn station_and_radius(p: [f64; 3], o: [f64; 3], k: [f64; 3]) -> (f64, f64) {
    let d = sub(p, o);
    let s = dot(d, k);
    (s, norm(sub(d, scale(k, s))))
}

/// The analytic geometry of a face's surface, its outward-normal sign, and the
/// surface itself for the generic normal fallback.
fn face_geometry(model: &BRepModel, face_id: u32) -> Option<(FaceGeometry, f64)> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let sign = face.orientation.sign();
    let any = surface.as_any();
    let g = if let Some(p) = any.downcast_ref::<Plane>() {
        FaceGeometry::Plane {
            origin: p3(p.origin),
            normal: scale(unit(v3(p.normal))?, sign),
        }
    } else if let Some(c) = any.downcast_ref::<Cylinder>() {
        FaceGeometry::Cylinder {
            origin: p3(c.origin),
            axis: unit(v3(c.axis))?,
            radius: c.radius,
        }
    } else if let Some(k) = any.downcast_ref::<Cone>() {
        FaceGeometry::Cone {
            apex: p3(k.apex),
            axis: unit(v3(k.axis))?,
            half_angle: k.half_angle,
        }
    } else if let Some(s) = any.downcast_ref::<Sphere>() {
        FaceGeometry::Sphere {
            center: p3(s.center),
            radius: s.radius,
        }
    } else if let Some(t) = any.downcast_ref::<Torus>() {
        FaceGeometry::Torus {
            center: p3(t.center),
            axis: unit(v3(t.axis))?,
            major_radius: t.major_radius,
            minor_radius: t.minor_radius,
        }
    } else {
        FaceGeometry::Other {
            type_name: surface.type_name().to_string(),
        }
    };
    Some((g, sign))
}

/// Outward unit normal of a face at a point on it. Closed-form for planes,
/// cylinders and cones (the surface conventions: a cylinder's native normal is
/// `+radial`; a cone's is `radialÃƒâ€šÃ‚Â·cos ÃƒÅ½Ã‚Â± ÃƒÂ¢Ã‹â€ Ã¢â‚¬â„¢ axisÃƒâ€šÃ‚Â·sin ÃƒÅ½Ã‚Â±` on its `v > 0` nappe);
/// the surface's own projection + normal for anything else.
fn outward_normal(
    model: &BRepModel,
    face_id: u32,
    geom: &FaceGeometry,
    sign: f64,
    p: [f64; 3],
) -> Option<[f64; 3]> {
    match geom {
        FaceGeometry::Plane { normal, .. } => Some(*normal),
        FaceGeometry::Cylinder { origin, axis, .. } => {
            let d = sub(p, *origin);
            let radial = unit(sub(d, scale(*axis, dot(d, *axis))))?;
            Some(scale(radial, sign))
        }
        FaceGeometry::Cone {
            apex,
            axis,
            half_angle,
        } => {
            let d = sub(p, *apex);
            let s = dot(d, *axis);
            let radial = unit(sub(d, scale(*axis, s)))?;
            let nappe = if s >= 0.0 { 1.0 } else { -1.0 };
            let native = sub(
                scale(radial, nappe * half_angle.cos()),
                scale(*axis, half_angle.sin()),
            );
            Some(scale(unit(native)?, sign))
        }
        _ => {
            let face = model.faces.get(face_id)?;
            let surface = model.surfaces.get(face.surface_id)?;
            let pt = Point3::new(p[0], p[1], p[2]);
            let (u, v) = surface.closest_point(&pt, Tolerance::default()).ok()?;
            let n = surface.normal_at(u, v).ok()?;
            unit(scale(v3(n), sign))
        }
    }
}

/// Sample points of an edge at `EDGE_SAMPLES + 1` evenly spaced parameters.
fn edge_samples(model: &BRepModel, edge_id: u32) -> Vec<[f64; 3]> {
    let Some(edge) = model.edges.get(edge_id) else {
        return Vec::new();
    };
    (0..=EDGE_SAMPLES)
        .filter_map(|k| {
            edge.evaluate(k as f64 / EDGE_SAMPLES as f64, &model.curves)
                .ok()
                .map(p3)
        })
        .collect()
}

/// Is the edge a circle (or arc of one) about the axis `(o, k)`? Returns its
/// station and radius when every sample shares both.
pub(crate) fn rim_circle(
    model: &BRepModel,
    edge_id: u32,
    o: [f64; 3],
    k: [f64; 3],
) -> Option<RimCircle> {
    let pts = edge_samples(model, edge_id);
    let first = pts.first()?;
    let (s0, r0) = station_and_radius(*first, o, k);
    let tol = REL_TOL * r0.max(1.0) * 10.0;
    for p in &pts[1..] {
        let (s, r) = station_and_radius(*p, o, k);
        if (s - s0).abs() > tol || (r - r0).abs() > tol {
            return None;
        }
    }
    (r0 > tol).then_some(RimCircle {
        station: s0,
        radius: r0,
    })
}

/// Do two face geometries describe the same analytic surface?
pub(crate) fn same_surface(a: &FaceGeometry, b: &FaceGeometry) -> bool {
    let line_eq = |o1: [f64; 3], k1: [f64; 3], o2: [f64; 3], k2: [f64; 3], tol: f64| {
        dot(k1, k2).abs() > 1.0 - 1e-9 && station_and_radius(o2, o1, k1).1 <= tol
    };
    match (a, b) {
        (
            FaceGeometry::Plane {
                origin: o1,
                normal: n1,
            },
            FaceGeometry::Plane {
                origin: o2,
                normal: n2,
            },
        ) => dot(*n1, *n2) > 1.0 - 1e-9 && dot(sub(*o2, *o1), *n1).abs() <= REL_TOL,
        (
            FaceGeometry::Cylinder {
                origin: o1,
                axis: k1,
                radius: r1,
            },
            FaceGeometry::Cylinder {
                origin: o2,
                axis: k2,
                radius: r2,
            },
        ) => {
            let tol = REL_TOL * r1.max(1.0);
            (r1 - r2).abs() <= tol && line_eq(*o1, *k1, *o2, *k2, tol)
        }
        (
            FaceGeometry::Cone {
                apex: a1,
                axis: k1,
                half_angle: h1,
            },
            FaceGeometry::Cone {
                apex: a2,
                axis: k2,
                half_angle: h2,
            },
        ) => {
            (h1 - h2).abs() <= 1e-9
                && dot(*k1, *k2) > 1.0 - 1e-9
                && norm(sub(*a1, *a2)) <= REL_TOL * norm(*a1).max(1.0)
        }
        _ => false,
    }
}

/// Every edge of every loop of a face, in loop order (a seam edge appears
/// twice in one face).
fn face_edges(model: &BRepModel, face_id: u32) -> Vec<u32> {
    let Some(face) = model.faces.get(face_id) else {
        return Vec::new();
    };
    face.all_loops()
        .into_iter()
        .filter_map(|l| model.loops.get(l))
        .flat_map(|l| l.edges.iter().copied())
        .collect()
}

/// Material side and station range of a cylinder / cone face.
fn revolution_attributes(
    model: &BRepModel,
    face_id: u32,
    geom: &FaceGeometry,
    sign: f64,
) -> (Option<bool>, Option<[f64; 2]>) {
    let Some((o, k)) = (match geom {
        FaceGeometry::Cylinder { .. } | FaceGeometry::Cone { .. } => geom.axis_line(),
        _ => None,
    }) else {
        return (None, None);
    };
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut cavity = None;
    for e in face_edges(model, face_id) {
        for p in edge_samples(model, e) {
            let (s, r) = station_and_radius(p, o, k);
            lo = lo.min(s);
            hi = hi.max(s);
            if cavity.is_none() && r > 1e-9 {
                if let Some(n) = outward_normal(model, face_id, geom, sign, p) {
                    let radial = sub(sub(p, o), scale(k, s));
                    let d = dot(n, radial);
                    if d.abs() > 1e-12 {
                        cavity = Some(d < 0.0);
                    }
                }
            }
        }
    }
    // A cone face whose every edge lies at ONE station is bounded by a single
    // rim circle, so it is the cap that closes to the apex (a drill point):
    // its station range runs to the apex, which no edge samples.
    if matches!(geom, FaceGeometry::Cone { .. }) && hi >= lo {
        let span_tol = REL_TOL * hi.abs().max(lo.abs()).max(1.0) * 10.0;
        if hi - lo <= span_tol {
            lo = lo.min(0.0);
            hi = hi.max(0.0);
        }
    }
    let range = (hi >= lo).then_some([lo, hi]);
    (cavity, range)
}

/// The direction leaving a circular rim edge INTO a cylinder / cone face, at
/// the rim point `m`, or `None` when the edge is not one of the face's rims.
fn into_revolution_face(
    model: &BRepModel,
    edge_id: u32,
    geom: &FaceGeometry,
    range: [f64; 2],
    m: [f64; 3],
) -> Option<[f64; 3]> {
    let (o, k) = geom.axis_line()?;
    let rim = rim_circle(model, edge_id, o, k)?;
    let span = range[1] - range[0];
    let tol = REL_TOL * span.max(1.0) * 10.0;
    // The rim must bound the face's station range at one end.
    let toward = if (rim.station - range[0]).abs() <= tol {
        1.0
    } else if (rim.station - range[1]).abs() <= tol {
        -1.0
    } else {
        return None;
    };
    if span <= tol {
        return None;
    }
    match geom {
        FaceGeometry::Cylinder { .. } => Some(scale(k, toward)),
        FaceGeometry::Cone { apex, .. } => {
            // Along the generator, oriented so the station moves toward the
            // face's interior.
            let g = unit(sub(m, *apex))?;
            let gk = dot(g, k);
            if gk.abs() < 1e-12 {
                return None;
            }
            Some(scale(g, toward * gk.signum()))
        }
        _ => None,
    }
}

/// Label one two-face edge.
fn label_arc(
    model: &BRepModel,
    edge_id: u32,
    fa: u32,
    fb: u32,
    nodes: &BTreeMap<u32, (FaceGeometry, f64, Option<[f64; 2]>)>,
) -> (EdgeConvexity, ConvexityDerivation) {
    if fa == fb {
        return (EdgeConvexity::Seam, ConvexityDerivation::Structural);
    }
    let (Some(a), Some(b)) = (nodes.get(&fa), nodes.get(&fb)) else {
        return (EdgeConvexity::Undetermined, ConvexityDerivation::Structural);
    };
    let Some(m) = model
        .edges
        .get(edge_id)
        .and_then(|e| e.evaluate(0.5, &model.curves).ok())
        .map(p3)
    else {
        return (EdgeConvexity::Undetermined, ConvexityDerivation::Structural);
    };
    // Analytic path: one side is a cylinder / cone whose rim this edge is.
    for (rev, rev_id, other, other_id) in [(a, fa, b, fb), (b, fb, a, fa)] {
        let Some(range) = rev.2 else {
            continue;
        };
        let Some(t_into) = into_revolution_face(model, edge_id, &rev.0, range, m) else {
            continue;
        };
        let (Some(n_rev), Some(n_other)) = (
            outward_normal(model, rev_id, &rev.0, rev.1, m),
            outward_normal(model, other_id, &other.0, other.1, m),
        ) else {
            continue;
        };
        if norm(cross(n_rev, n_other)) < TANGENT_SIN_TOL && dot(n_rev, n_other) > 0.0 {
            return (EdgeConvexity::Tangent, ConvexityDerivation::Analytic);
        }
        let s = dot(n_other, t_into);
        if s.abs() < TANGENT_SIN_TOL {
            continue;
        }
        let c = if s < 0.0 {
            EdgeConvexity::Convex
        } else {
            EdgeConvexity::Concave
        };
        return (c, ConvexityDerivation::Analytic);
    }
    // Fallback: the kernel's dihedral classifier on this face pair.
    match crate::operations::edge_classification::classify_face_pair(model, edge_id, fa, fb) {
        Ok(c) => match (c.dihedral_angle, c.convexity) {
            (Some(_), 1) => (EdgeConvexity::Convex, ConvexityDerivation::Dihedral),
            (Some(_), -1) => (EdgeConvexity::Concave, ConvexityDerivation::Dihedral),
            (Some(_), _) => (EdgeConvexity::Tangent, ConvexityDerivation::Dihedral),
            (None, _) => (EdgeConvexity::Undetermined, ConvexityDerivation::Dihedral),
        },
        Err(_) => (EdgeConvexity::Undetermined, ConvexityDerivation::Dihedral),
    }
}

/// Build the attributed adjacency graph of `solid_id`. An unknown solid yields
/// an empty graph.
pub fn attributed_adjacency_graph(
    model: &BRepModel,
    solid_id: SolidId,
) -> AttributedAdjacencyGraph {
    let mut graph = AttributedAdjacencyGraph {
        solid: solid_id,
        nodes: Vec::new(),
        arcs: Vec::new(),
        boundary_edges: Vec::new(),
        non_manifold_edges: Vec::new(),
    };
    let Some(solid) = model.solids.get(solid_id) else {
        return graph;
    };

    // Faces of the solid's own shells, ascending and unique.
    let mut faces: Vec<u32> = Vec::new();
    for sh in solid.all_shells() {
        if let Some(shell) = model.shells.get(sh) {
            faces.extend(shell.faces.iter().copied());
        }
    }
    faces.sort_unstable();
    faces.dedup();

    // Per-face attributes and the edge ÃƒÂ¢Ã¢â‚¬Â Ã¢â‚¬â„¢ face-side incidence.
    let mut attrs: BTreeMap<u32, (FaceGeometry, f64, Option<[f64; 2]>)> = BTreeMap::new();
    let mut cavities: BTreeMap<u32, Option<bool>> = BTreeMap::new();
    let mut loops: BTreeMap<u32, usize> = BTreeMap::new();
    let mut incidence: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &f in &faces {
        let Some((geom, sign)) = face_geometry(model, f) else {
            continue;
        };
        let (cavity, range) = revolution_attributes(model, f, &geom, sign);
        loops.insert(
            f,
            model
                .faces
                .get(f)
                .map(|x| 1 + x.inner_loops.len())
                .unwrap_or(0),
        );
        for e in face_edges(model, f) {
            incidence.entry(e).or_default().push(f);
        }
        cavities.insert(f, cavity);
        attrs.insert(f, (geom, sign, range));
    }

    let mut node_arcs: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (&e, sides) in &incidence {
        match sides.len() {
            0 => {}
            1 => graph.boundary_edges.push(e),
            2 => {
                let (fa, fb) = (sides[0].min(sides[1]), sides[0].max(sides[1]));
                let (convexity, derivation) = label_arc(model, e, fa, fb, &attrs);
                let same = fa == fb
                    || match (attrs.get(&fa), attrs.get(&fb)) {
                        (Some(a), Some(b)) => same_surface(&a.0, &b.0),
                        _ => false,
                    };
                let idx = graph.arcs.len();
                graph.arcs.push(AagArc {
                    edge: e,
                    faces: [fa, fb],
                    convexity,
                    derivation,
                    same_surface: same,
                });
                node_arcs.entry(fa).or_default().push(idx);
                if fb != fa {
                    node_arcs.entry(fb).or_default().push(idx);
                }
            }
            _ => graph.non_manifold_edges.push(e),
        }
    }

    for (f, (geom, _, range)) in attrs {
        graph.nodes.push(AagNode {
            face: f,
            geometry: geom,
            cavity: cavities.get(&f).copied().flatten(),
            station_range: range,
            loop_count: loops.get(&f).copied().unwrap_or(0),
            arcs: node_arcs.remove(&f).unwrap_or_default(),
        });
    }
    graph
}
