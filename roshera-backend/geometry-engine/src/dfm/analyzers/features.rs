//! `concave_features` — the process-agnostic census of a solid's INSIDE
//! geometry that machining, casting and sheet rules all read: sharp concave
//! edges, cavity cylinders (bores, corner radii, slot ends, bend insides),
//! and every edge whose convexity could not be derived.
//!
//! ## Built on the attributed adjacency graph, never a second classifier
//!
//! Convexity comes from [`crate::readable::adjacency`] (exact on analytic
//! rims, the kernel's dihedral classifier elsewhere). The one difference
//! from [`super::blend_radius`]'s sharp-corner arm is honesty about the
//! UNKNOWN: `blend_radius` skips an edge the classifier errors on, so an
//! unclassifiable concave edge could vanish from a corner rule. Here every
//! [`EdgeConvexity::Undetermined`] arc is reported as
//! [`ConcaveFeatures::undetermined`] and the consuming rule refuses on it.
//!
//! ## Cavity cylinders: hole or corner is decided by SWEEP, exactly
//!
//! A cavity cylinder (material surrounding the axis — a bore wall, a
//! pocket's vertical corner radius, a slot end, a bend's inside) is merged
//! across every face of the solid on the same surface (the boolean splits
//! one wall into several faces; [`crate::readable::adjacency`]'s own
//! `same_surface`). Its angular SWEEP is read exactly off its circular rim
//! edges: an `Arc` edge covers `|sweep_angle| · |Δt|` (the arc is
//! `angle = start + sweep·t`, `primitives/curve.rs`), a `Circle` edge
//! `2π · |Δt|`; spans are summed per rim station across the merged faces.
//! A wall whose rims close a full turn (`2π`) is a HOLE; a partial wall is a
//! corner radius / slot end / bend — a tool or a bend forms it. A wall with
//! no circular rim at all has an unknown sweep (`None`), and consumers treat
//! it as an unresolved hole, never as a corner. This is the gate that keeps
//! a 1 mm pocket-corner fillet from reading as a Ø2 hole.

use std::collections::BTreeMap;

use crate::dfm::report::DfmError;
use crate::primitives::curve::{Arc, Circle, Line};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::readable::adjacency::{
    attributed_adjacency_graph, same_surface, AttributedAdjacencyGraph, EdgeConvexity, FaceGeometry,
};

/// A full turn, with a round-off allowance on summed rim spans.
const FULL_TURN: f64 = std::f64::consts::TAU - 1e-9;
/// Parallel-direction threshold on `|a·b|` (the hole recogniser's).
pub(crate) const PARALLEL_COS: f64 = 1.0 - 1e-9;
/// Perpendicular-direction threshold on `|a·b|`.
pub(crate) const PERPENDICULAR_COS: f64 = 1e-9;

/// The curve a concave edge runs along.
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeShape {
    /// A straight edge, unit `direction`.
    Line { direction: [f64; 3] },
    /// A circle or arc: `center`, unit plane `normal`, `radius`.
    Rim {
        center: [f64; 3],
        normal: [f64; 3],
        radius: f64,
    },
    /// Any other curve (intersection NURBS, ellipse, …).
    Other,
}

/// A sharp (unblended) concave edge: the material wedge exceeds 180°.
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveEdge {
    pub edge: EdgeId,
    /// The two faces meeting at it, ascending.
    pub faces: [FaceId; 2],
    pub shape: EdgeShape,
}

/// An edge whose convexity could not be derived (degenerate or failed
/// classification) — never assumed convex.
#[derive(Debug, Clone, PartialEq)]
pub struct UndeterminedEdge {
    pub edge: EdgeId,
    pub faces: [FaceId; 2],
}

/// Every face of the solid on one cavity cylinder surface, merged.
#[derive(Debug, Clone, PartialEq)]
pub struct CavityCylinder {
    /// Faces on the surface, ascending.
    pub faces: Vec<FaceId>,
    pub origin: [f64; 3],
    /// Unit axis as stored on the surface.
    pub axis: [f64; 3],
    pub radius: f64,
    /// `[min, max]` station along `axis` from `origin`, union over faces.
    pub station: [f64; 2],
    /// Largest angular coverage of any rim station, radians; `None` when
    /// the wall has no circular rim edge to read it from.
    pub sweep: Option<f64>,
    /// True when some face of the wall meets a face of ANOTHER surface
    /// tangentially (a blend transition).
    pub tangent: bool,
}

impl CavityCylinder {
    /// Rims close a full turn: a hole wall, not a corner.
    pub fn is_full_turn(&self) -> bool {
        self.sweep.is_some_and(|s| s >= FULL_TURN)
    }
    /// Rims PROVABLY cover less than a full turn: a corner, slot end or bend.
    pub fn is_partial(&self) -> bool {
        self.sweep.is_some_and(|s| s < FULL_TURN)
    }
    /// Axial length of the merged wall.
    pub fn length(&self) -> f64 {
        self.station[1] - self.station[0]
    }
}

/// The census.
#[derive(Debug, Clone)]
pub struct ConcaveFeatures {
    /// The adjacency graph the census was read from (consumers look up
    /// face geometry in it rather than rebuilding it).
    pub graph: AttributedAdjacencyGraph,
    pub sharp_edges: Vec<ConcaveEdge>,
    pub undetermined: Vec<UndeterminedEdge>,
    pub cavity_cylinders: Vec<CavityCylinder>,
    /// Faces the adjacency graph reports as `Other` that were read exactly
    /// as straight-spine constant-radius fillets ([`fillet_cylinder`]).
    pub resolved_fillets: std::collections::BTreeSet<FaceId>,
}

impl ConcaveFeatures {
    /// Analytic geometry of a face of the solid.
    pub fn geometry(&self, face: FaceId) -> Option<&FaceGeometry> {
        self.graph.node(face).map(|n| &n.geometry)
    }
}

fn unit(v: [f64; 3]) -> Option<[f64; 3]> {
    let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    (n > 0.0 && n.is_finite()).then(|| [v[0] / n, v[1] / n, v[2] / n])
}

pub(crate) fn dot3(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn edge_shape(model: &BRepModel, edge_id: EdgeId) -> EdgeShape {
    let Some(edge) = model.edges.get(edge_id) else {
        return EdgeShape::Other;
    };
    let Some(curve) = model.curves.get(edge.curve_id) else {
        return EdgeShape::Other;
    };
    let any = curve.as_any();
    if let Some(line) = any.downcast_ref::<Line>() {
        let d = line.end - line.start;
        return unit([d.x, d.y, d.z])
            .map_or(EdgeShape::Other, |direction| EdgeShape::Line { direction });
    }
    if let Some(arc) = any.downcast_ref::<Arc>() {
        return EdgeShape::Rim {
            center: [arc.center.x, arc.center.y, arc.center.z],
            normal: [arc.normal.x, arc.normal.y, arc.normal.z],
            radius: arc.radius,
        };
    }
    if let Some(c) = any.downcast_ref::<Circle>() {
        let (o, n) = (c.center(), c.normal());
        return EdgeShape::Rim {
            center: [o.x, o.y, o.z],
            normal: [n.x, n.y, n.z],
            radius: c.radius(),
        };
    }
    EdgeShape::Other
}

/// `(station along axis, span radians)` of a circular rim edge about the
/// axis `(o, k)`, or `None` when the edge is not such a rim.
pub(crate) fn rim_span(
    model: &BRepModel,
    edge_id: EdgeId,
    o: [f64; 3],
    k: [f64; 3],
) -> Option<(f64, f64)> {
    let edge = model.edges.get(edge_id)?;
    let curve = model.curves.get(edge.curve_id)?;
    let any = curve.as_any();
    let dt = (edge.param_range.end - edge.param_range.start).abs();
    let (center, normal, span) = if let Some(arc) = any.downcast_ref::<Arc>() {
        (arc.center, arc.normal, arc.sweep_angle.abs() * dt)
    } else if let Some(c) = any.downcast_ref::<Circle>() {
        (c.center(), c.normal(), std::f64::consts::TAU * dt)
    } else {
        return None;
    };
    let n = [normal.x, normal.y, normal.z];
    if dot3(n, k).abs() < PARALLEL_COS {
        return None;
    }
    let d = [center.x - o[0], center.y - o[1], center.z - o[2]];
    let s = dot3(d, k);
    let off = [d[0] - k[0] * s, d[1] - k[1] * s, d[2] - k[2] * s];
    let tol = 1e-6 * o.iter().fold(1.0f64, |m, v| m.max(v.abs()));
    (dot3(off, off).sqrt() <= tol).then_some((s, span))
}

/// Build the census for `solid_id`.
pub fn concave_features(model: &BRepModel, solid_id: SolidId) -> Result<ConcaveFeatures, DfmError> {
    if model.solids.get(solid_id).is_none() {
        return Err(DfmError::DanglingSolidRef { solid: solid_id });
    }
    let graph = attributed_adjacency_graph(model, solid_id);

    let mut sharp_edges = Vec::new();
    let mut undetermined = Vec::new();
    for arc in &graph.arcs {
        if arc.faces[0] == arc.faces[1] {
            continue; // a seam: one face meeting itself
        }
        match arc.convexity {
            EdgeConvexity::Concave if !arc.same_surface => sharp_edges.push(ConcaveEdge {
                edge: arc.edge,
                faces: arc.faces,
                shape: edge_shape(model, arc.edge),
            }),
            EdgeConvexity::Undetermined => undetermined.push(UndeterminedEdge {
                edge: arc.edge,
                faces: arc.faces,
            }),
            _ => {}
        }
    }

    // Cavity cylinders merged by surface.
    let mut groups: Vec<(CavityCylinder, FaceGeometry)> = Vec::new();
    for n in &graph.nodes {
        if n.cavity != Some(true) {
            continue;
        }
        let FaceGeometry::Cylinder {
            origin,
            axis,
            radius,
        } = &n.geometry
        else {
            continue;
        };
        let Some(range) = n.station_range else {
            continue;
        };
        if let Some((g, _)) = groups
            .iter_mut()
            .find(|(_, geom)| same_surface(geom, &n.geometry))
        {
            g.faces.push(n.face);
            g.station = [g.station[0].min(range[0]), g.station[1].max(range[1])];
        } else {
            groups.push((
                CavityCylinder {
                    faces: vec![n.face],
                    origin: *origin,
                    axis: *axis,
                    radius: *radius,
                    station: range,
                    sweep: None,
                    tangent: false,
                },
                n.geometry.clone(),
            ));
        }
    }

    let mut cavity_cylinders = Vec::with_capacity(groups.len());
    for (mut g, _) in groups {
        g.faces.sort_unstable();
        let mut spans: BTreeMap<i64, f64> = BTreeMap::new();
        // An edge two faces of the same wall share (a split along a rim)
        // is one piece of rim, counted once.
        let mut seen_edges: std::collections::BTreeSet<EdgeId> = Default::default();
        let tol = 1e-6 * g.radius.max(1.0);
        for &f in &g.faces {
            let Some(face) = model.faces.get(f) else {
                continue;
            };
            let mut loops = vec![face.outer_loop];
            loops.extend_from_slice(&face.inner_loops);
            for lid in loops {
                let Some(lp) = model.loops.get(lid) else {
                    continue;
                };
                for &e in &lp.edges {
                    if !seen_edges.insert(e) {
                        continue;
                    }
                    if let Some((s, span)) = rim_span(model, e, g.origin, g.axis) {
                        // Bucket stations to the tolerance grid.
                        let key = (s / tol).round() as i64;
                        *spans.entry(key).or_insert(0.0) += span;
                    }
                }
            }
            for (a, other) in graph.neighbours(f) {
                if a.convexity == EdgeConvexity::Tangent
                    && !a.same_surface
                    && !g.faces.contains(&other)
                {
                    g.tangent = true;
                }
            }
        }
        g.sweep = spans.values().copied().reduce(f64::max);
        cavity_cylinders.push(g);
    }

    // Constant-radius fillets on a STRAIGHT spine are exact cylinders the
    // adjacency graph reports as `Other` (their surface is a
    // `CylindricalFillet`, not a `Cylinder`). Read them exactly.
    let mut resolved_fillets = std::collections::BTreeSet::new();
    for n in &graph.nodes {
        if !matches!(n.geometry, FaceGeometry::Other { .. }) {
            continue;
        }
        let Some(fc) = fillet_cylinder(model, n.face) else {
            continue;
        };
        resolved_fillets.insert(n.face);
        if fc.cavity {
            cavity_cylinders.push(CavityCylinder {
                faces: vec![n.face],
                origin: fc.origin,
                axis: fc.axis,
                radius: fc.radius,
                station: fc.station,
                sweep: Some(fc.sweep),
                tangent: true,
            });
        }
    }
    cavity_cylinders.sort_by_key(|c| c.faces.first().copied().unwrap_or(FaceId::MAX));

    Ok(ConcaveFeatures {
        graph,
        sharp_edges,
        undetermined,
        cavity_cylinders,
        resolved_fillets,
    })
}

/// A `CylindricalFillet` face read as the exact cylinder it is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilletCylinder {
    pub origin: [f64; 3],
    pub axis: [f64; 3],
    pub radius: f64,
    /// Material surrounds the spine (a concave / inside fillet).
    pub cavity: bool,
    /// Station range along `axis` from `origin`, read off the trim edges.
    pub station: [f64; 2],
    /// Angular span of the fillet, radians (always < π by construction).
    pub sweep: f64,
}

/// Read a face whose surface is a constant-radius `CylindricalFillet` on a
/// STRAIGHT spine (a `Line`, constant axis field) as an exact cylinder:
/// axis = the spine, radius = the fillet radius. The material side is read
/// from the face's own OUTWARD normal at the surface's mid-parameter
/// against the radial direction there (never assumed from the
/// orientation flag: the fillet surface's native normal convention is its
/// own). `None` for any other surface, a curved spine, or a failed
/// evaluation.
pub fn fillet_cylinder(model: &BRepModel, face_id: FaceId) -> Option<FilletCylinder> {
    use crate::primitives::fillet_surfaces::CylindricalFillet;
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let f = surface.as_any().downcast_ref::<CylindricalFillet>()?;
    let line = f.spine.as_any().downcast_ref::<Line>()?;
    let d = line.end - line.start;
    let axis = unit([d.x, d.y, d.z])?;
    if f.axis_field
        .iter()
        .any(|a| dot3(unit([a.x, a.y, a.z]).unwrap_or([0.0; 3]), axis).abs() < PARALLEL_COS)
    {
        return None;
    }
    let origin = [line.start.x, line.start.y, line.start.z];
    let p = surface.point_at(0.5, 0.5).ok()?;
    let n = surface.normal_at(0.5, 0.5).ok()?;
    let rel = [p.x - origin[0], p.y - origin[1], p.z - origin[2]];
    let s = dot3(rel, axis);
    let radial = [
        rel[0] - axis[0] * s,
        rel[1] - axis[1] * s,
        rel[2] - axis[2] * s,
    ];
    let outward = dot3([n.x, n.y, n.z], radial) * face.orientation.sign();
    if outward.abs() <= 1e-12 {
        return None;
    }
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut loops = vec![face.outer_loop];
    loops.extend_from_slice(&face.inner_loops);
    for lid in loops {
        for &e in &model.loops.get(lid)?.edges {
            let edge = model.edges.get(e)?;
            for k in 0..=8 {
                let q = edge.evaluate(k as f64 / 8.0, &model.curves).ok()?;
                let v = dot3([q.x - origin[0], q.y - origin[1], q.z - origin[2]], axis);
                lo = lo.min(v);
                hi = hi.max(v);
            }
        }
    }
    let sweep = f
        .angle_span
        .iter()
        .map(|(a, b)| (b - a).abs())
        .fold(0.0f64, f64::max);
    (hi >= lo && sweep > 0.0).then_some(FilletCylinder {
        origin,
        axis,
        radius: f.radius,
        cavity: outward < 0.0,
        station: [lo, hi],
        sweep,
    })
}
