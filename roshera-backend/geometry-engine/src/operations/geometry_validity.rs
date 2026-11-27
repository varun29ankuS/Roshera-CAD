//! Geometric-validity checks the *topological* B-Rep validator cannot make.
//!
//! `validate_model_enhanced` and `brep_integrity` verify TOPOLOGY — loops close,
//! every edge is shared by exactly two faces, the Euler relation holds. None of
//! that sees GEOMETRY: a solid can satisfy every topological invariant while a
//! face is geometrically self-overlapping. That is exactly the
//! chamfer-crosses-fillet failure (#70): chamfering an edge that cuts through an
//! existing fillet leaves a planar end-face whose boundary loop carries the
//! fillet's arc, and that arc bulges *past* the new chamfer edge — so the face's
//! own boundary self-intersects. Topologically the solid is pristine; it just
//! isn't a real solid, and the only place it surfaces today is a silent
//! tessellation hole.
//!
//! [`self_overlapping_planar_faces`] catches that class directly, so an operation
//! can reject a self-overlapping result instead of emitting it. It is restricted
//! to PLANAR faces: a planar face's boundary must project to a simple polygon in
//! its own plane, so a projected self-intersection is an unambiguous defect.
//! Curved faces are skipped — projecting a curved boundary to a single plane can
//! cross without the surface itself self-overlapping, which would false-positive.

use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Boundary samples emitted per *curved* loop edge when building the projected
/// polygon. A straight edge contributes a single sample (its start); only curved
/// edges need densifying to expose a bulge that crosses a neighbour.
const CURVED_SAMPLES: usize = 16;

/// A curved boundary edge's chords are halved until each lies within this
/// distance of the curve (mm)...
const SAGITTA_TOL: f64 = 1e-5;

/// ...or this many halvings deep (2^14 per initial chord at most).
const SUBDIVISION_DEPTH: usize = 14;

/// The planar faces of `solid` whose outer boundary loop is geometrically
/// self-intersecting (two non-adjacent boundary segments properly cross when the
/// loop is projected into the face's own plane). An empty result means every
/// planar face is a simple region — the geometric-validity precondition for a
/// real solid. A non-empty result is a self-overlapping solid that is
/// nonetheless topologically clean (the #70 class).
pub fn self_overlapping_planar_faces(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut bad = Vec::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return bad;
    };
    let mut shells = vec![solid.outer_shell];
    shells.extend(solid.inner_shells.iter().copied());
    shells.extend(solid.peer_shells.iter().copied());
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            if planar_face_self_intersects(model, fid) {
                bad.push(fid);
            }
        }
    }
    bad
}

/// Is `face` planar (a `Plane` surface)?
fn is_planar_face(model: &BRepModel, face: &crate::primitives::face::Face) -> bool {
    model
        .surfaces
        .get(face.surface_id)
        .map(|s| s.type_name() == "Plane")
        .unwrap_or(false)
}

/// Is `edge`'s curve a straight line?
fn is_line_edge(model: &BRepModel, edge: &crate::primitives::edge::Edge) -> bool {
    model
        .curves
        .get(edge.curve_id)
        .map(|c| c.type_name() == "Line")
        .unwrap_or(false)
}

/// The widest a vertex's tolerance zone is taken to be (mm): the importer's
/// ceiling for a tolerant edge (`EDGE_GAP_ACCEPT_CEILING` in the STEP
/// reader). A vertex or edge claiming more is held to this.
pub const TOLERANT_VERTEX_CEILING: f64 = 0.05;

/// Most crossings of one outline looked at before it is simply called
/// self-overlapping.
const MAX_CROSSINGS_EXAMINED: usize = 64;

/// Two consecutive edges of a planar face's loop crossing INSIDE the
/// tolerance zone of the vertex they share. Within that zone the two curves'
/// order is not defined -- the vertex is where they meet, to that tolerance
/// -- so this is the vertex, not an overlap of the face. It is never silent:
/// the validator reports each one.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TolerantVertexCrossing {
    pub face: FaceId,
    pub vertex: crate::primitives::vertex::VertexId,
    /// How far from the vertex the two edges cross (mm).
    pub distance: f64,
    /// The radius of the zone it was judged against: the largest tolerance
    /// carried by the vertex and the two edges, held to
    /// [`TOLERANT_VERTEX_CEILING`].
    pub tolerance: f64,
}

/// What a planar face's outer loop does when walked: whether it overlaps
/// itself, and the crossings that are only a tolerant vertex.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlanarOutlineFinding {
    /// Two parts of the boundary pass through each other where no vertex
    /// tolerance accounts for it.
    pub self_overlaps: bool,
    /// Crossings of consecutive edges inside their shared vertex's zone.
    pub at_tolerant_vertices: Vec<TolerantVertexCrossing>,
}

/// Whether planar face `fid`'s outer loop crosses itself (its boundary sampled
/// in loop order, curves followed to within `SAGITTA_TOL`). The per-face test
/// behind [`self_overlapping_planar_faces`]. A crossing of two consecutive
/// edges inside their shared vertex's tolerance zone is not counted (see
/// [`planar_face_outline`]).
pub fn planar_face_self_intersects(model: &BRepModel, fid: FaceId) -> bool {
    planar_face_outline(model, fid).self_overlaps
}

/// The crossings of planar face `fid`'s outline that are only a tolerant
/// vertex: consecutive edges, inside the zone. Empty for an exact face.
pub fn tolerant_vertex_crossings(model: &BRepModel, fid: FaceId) -> Vec<TolerantVertexCrossing> {
    planar_face_outline(model, fid).at_tolerant_vertices
}

/// One sample of a loop's outline: the point, the loop edge it is on, and
/// whether it is that edge's own END (the segment leaving it is then the
/// JOINT to the next edge's start, not part of the edge).
#[derive(Clone, Copy)]
struct OutlinePoint {
    point: Point3,
    edge: usize,
    is_end: bool,
}

/// Walk planar face `fid`'s outer loop and judge its outline.
///
/// Where one edge's curve ends a hair from where the next one's starts, the
/// outline has to be closed there somehow, and there are two honest ways:
///
/// * the established one: each edge is followed up to where the NEXT edge
///   starts, so the arriving edge's last chord is bent onto its neighbour;
/// * each edge is followed along ITS OWN curve to its own end, and the step
///   to the next edge's start is a segment of its own, the joint at their
///   vertex.
///
/// On an exact model the two are the same outline. On a tolerant one each
/// can show a crossing the other does not: the first swings a straight edge
/// whose neighbour begins 23 micrometres off its end along its whole length,
/// across a third edge 0.07 mm from the far corner (CADGenBench part 201);
/// the second keeps two edges that leave a needle-sharp corner a micrometre
/// apart running through each other for 0.06 mm (part 250). A crossing that
/// appears only under one way of closing a joint is that joint, not the
/// face. So the established outline is judged first and, if it does not
/// cross, the face is simple, as it always was. Only if it crosses is the
/// second outline judged, and the face overlaps itself only if that one
/// does too.
///
/// In the second outline two segments that properly cross are an overlap,
/// with one exception: both belong to two CONSECUTIVE edges (or the joint
/// between them) and they cross within the tolerance zone of the vertex
/// those edges share. That zone's radius is the largest tolerance the
/// vertex and the two edges carry, never more than
/// [`TOLERANT_VERTEX_CEILING`]. Edges that are not neighbours, an edge
/// crossing itself, and any crossing outside the zone are overlaps.
pub fn planar_face_outline(model: &BRepModel, fid: FaceId) -> PlanarOutlineFinding {
    let established = planar_face_outline_closed(model, fid, JointClosing::OntoNextStart);
    if !established.self_overlaps {
        return PlanarOutlineFinding::default();
    }
    planar_face_outline_closed(model, fid, JointClosing::OwnEnds)
}

/// The two ways the sampled outline of a loop is closed where one edge's
/// curve ends a hair from where the next one's starts (see
/// [`planar_face_outline`], which judges the first and then, only if it
/// crosses, the second).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JointClosing {
    /// Each edge is followed up to where the NEXT edge starts. Any crossing
    /// is an overlap.
    OntoNextStart,
    /// Each edge is followed to its own end and the step to the next edge's
    /// start is a segment of its own. Judged with the tolerant-vertex rule.
    OwnEnds,
}

/// [`planar_face_outline`] under ONE way of closing the joints, on its own.
/// The verdict on a face is [`planar_face_outline`]'s; this is the single
/// reading it is built from, exposed so each can be examined and tested.
pub fn planar_face_outline_closed(
    model: &BRepModel,
    fid: FaceId,
    closing: JointClosing,
) -> PlanarOutlineFinding {
    let own_ends = closing == JointClosing::OwnEnds;
    let mut finding = PlanarOutlineFinding::default();
    let Some(face) = model.faces.get(fid) else {
        return finding;
    };
    if !is_planar_face(model, face) {
        return finding;
    }
    let Some(lp) = model.loops.get(face.outer_loop) else {
        return finding;
    };

    let mut pts: Vec<OutlinePoint> = Vec::with_capacity(lp.edges.len() * 2);
    for (i, &eid) in lp.edges.iter().enumerate() {
        let fwd = lp.orientations.get(i).copied().unwrap_or(true);
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        let at = |s: f64| {
            edge.evaluate(if fwd { s } else { 1.0 - s }, &model.curves)
                .ok()
        };
        let body = |point: Point3| OutlinePoint {
            point,
            edge: i,
            is_end: false,
        };
        if is_line_edge(model, edge) {
            if let Some(p) = at(0.0) {
                pts.push(body(p));
            }
        } else {
            // A curved edge is followed until each chord lies within
            // SAGITTA_TOL of the curve. A fixed 16 chords made two curves that
            // meet at a shallow angle cross in the CHORD polygon although the
            // curves themselves do not (CADGenBench parts 131, 134, 249: a
            // false "self-overlaps" on a planar face).
            for j in 0..CURVED_SAMPLES {
                let (s0, s1) = (
                    j as f64 / CURVED_SAMPLES as f64,
                    (j + 1) as f64 / CURVED_SAMPLES as f64,
                );
                let (Some(p0), Some(p1)) = (at(s0), at(s1)) else {
                    continue;
                };
                pts.push(body(p0));
                // Depth-first subdivision of [s0, s1], emitting interior points
                // in order.
                let mut stack = vec![(s0, p0, s1, p1, 0usize)];
                let mut interior: Vec<(f64, Point3)> = Vec::new();
                while let Some((a, pa, b, pb, depth)) = stack.pop() {
                    let m = 0.5 * (a + b);
                    let Some(pm) = at(m) else {
                        continue;
                    };
                    let mid = Point3::new(
                        0.5 * (pa.x + pb.x),
                        0.5 * (pa.y + pb.y),
                        0.5 * (pa.z + pb.z),
                    );
                    if depth < SUBDIVISION_DEPTH && (pm - mid).magnitude() > SAGITTA_TOL {
                        stack.push((a, pa, m, pm, depth + 1));
                        stack.push((m, pm, b, pb, depth + 1));
                        interior.push((m, pm));
                    }
                }
                interior.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
                pts.extend(interior.into_iter().map(|(_, p)| body(p)));
            }
        }
        // The edge's own end; the segment from it is the joint.
        if let Some(p) = at(1.0).filter(|_| own_ends) {
            pts.push(OutlinePoint {
                point: p,
                edge: i,
                is_end: true,
            });
        }
    }
    let m = pts.len();
    if m < 4 {
        return finding; // a triangle (or less) cannot self-intersect
    }

    let ring: Vec<Point3> = pts.iter().map(|p| p.point).collect();
    // Projected along the face's own plane. (The outline's best-fit normal
    // was used here, and a symmetric bow-tie has none -- its two lobes
    // cancel -- so the plainest self-overlap of all was passed as "nothing
    // to project".) The best-fit normal remains the fallback.
    let plane_normal = model
        .surfaces
        .get(face.surface_id)
        .and_then(|s| s.normal_at(0.0, 0.0).ok())
        .and_then(|n| n.normalize().ok());
    let Some(normal) = plane_normal.or_else(|| newell_normal(&ring)) else {
        return finding; // degenerate / collinear — nothing to project
    };
    let (u_axis, v_axis) = plane_axes(normal);
    let origin = ring[0];
    let p2d: Vec<(f64, f64)> = ring
        .iter()
        .map(|p| {
            let r = *p - origin;
            (r.dot(&u_axis), r.dot(&v_axis))
        })
        .collect();

    let crossings = polygon_crossings(&p2d, MAX_CROSSINGS_EXAMINED);
    if crossings.len() >= MAX_CROSSINGS_EXAMINED {
        finding.self_overlaps = true;
        return finding;
    }
    let edges_in_loop = lp.edges.len();
    for (a, b) in crossings {
        let excused = if own_ends {
            crossing_at_tolerant_vertex(model, fid, lp, &pts, &p2d, a, b, edges_in_loop)
        } else {
            None
        };
        match excused {
            Some(excused) => finding.at_tolerant_vertices.push(excused),
            None => finding.self_overlaps = true,
        }
    }
    if finding.self_overlaps {
        // An overlapping face is reported as that; the vertex crossings on
        // it are not a separate finding.
        finding.at_tolerant_vertices.clear();
    }
    finding
}

/// If outline segments `a` and `b` (each named by its first point) cross
/// inside the tolerance zone of a vertex shared by the two consecutive loop
/// edges they belong to, that crossing; `None` when it is an overlap.
#[allow(clippy::too_many_arguments)]
fn crossing_at_tolerant_vertex(
    model: &BRepModel,
    fid: FaceId,
    lp: &crate::primitives::r#loop::Loop,
    pts: &[OutlinePoint],
    p2d: &[(f64, f64)],
    a: usize,
    b: usize,
    edges_in_loop: usize,
) -> Option<TolerantVertexCrossing> {
    let m = pts.len();
    let (pa, pb) = (pts[a], pts[b]);
    // Where the two segments cross, on segment `a`.
    let (a0, a1, b0, b1) = (p2d[a], p2d[(a + 1) % m], p2d[b], p2d[(b + 1) % m]);
    let denom = (a1.0 - a0.0) * (b1.1 - b0.1) - (a1.1 - a0.1) * (b1.0 - b0.0);
    if denom == 0.0 {
        return None;
    }
    let t = ((b0.0 - a0.0) * (b1.1 - b0.1) - (b0.1 - a0.1) * (b1.0 - b0.0)) / denom;
    let (s0, s1) = (pa.point, pts[(a + 1) % m].point);
    let crossing = s0 + (s1 - s0) * t.clamp(0.0, 1.0);

    // The joints the pair could be about: joint k lies between loop edge k
    // and loop edge k + 1, and holds edge k's body, the joint segment after
    // it, and edge k + 1's body.
    let in_joint =
        |p: OutlinePoint, k: usize| p.edge == k || (p.edge == (k + 1) % edges_in_loop && !p.is_end);
    let candidates = [pa.edge, pb.edge];
    for &k in &candidates {
        for k in [k, (k + edges_in_loop - 1) % edges_in_loop] {
            if !(in_joint(pa, k) && in_joint(pb, k)) {
                continue;
            }
            // Not one edge's body crossing itself.
            if pa.edge == pb.edge && !pa.is_end && !pb.is_end {
                continue;
            }
            let fwd = lp.orientations.get(k).copied().unwrap_or(true);
            let next = (k + 1) % edges_in_loop;
            let (Some(edge), Some(next_edge)) = (
                model.edges.get(*lp.edges.get(k)?),
                model.edges.get(*lp.edges.get(next)?),
            ) else {
                continue;
            };
            let vertex_id = if fwd {
                edge.end_vertex
            } else {
                edge.start_vertex
            };
            let Some(vertex) = model.vertices.get(vertex_id) else {
                continue;
            };
            let finite = |x: f64| if x.is_finite() { x } else { 0.0 };
            let zone = finite(vertex.tolerance)
                .max(finite(edge.tolerance))
                .max(finite(next_edge.tolerance))
                .min(TOLERANT_VERTEX_CEILING);
            let distance = (crossing - vertex.point()).magnitude();
            if distance <= zone {
                return Some(TolerantVertexCrossing {
                    face: fid,
                    vertex: vertex_id,
                    distance,
                    tolerance: zone,
                });
            }
        }
    }
    None
}

/// Newell's best-fit normal of a 3D point ring; `None` if degenerate.
fn newell_normal(pts: &[Point3]) -> Option<Vector3> {
    let n = pts.len();
    let mut nx = 0.0;
    let mut ny = 0.0;
    let mut nz = 0.0;
    for i in 0..n {
        let a = pts[i];
        let b = pts[(i + 1) % n];
        nx += (a.y - b.y) * (a.z + b.z);
        ny += (a.z - b.z) * (a.x + b.x);
        nz += (a.x - b.x) * (a.y + b.y);
    }
    Vector3::new(nx, ny, nz).normalize().ok()
}

/// Orthonormal (u, v) spanning the plane with the given `normal`.
fn plane_axes(normal: Vector3) -> (Vector3, Vector3) {
    // Pick a seed axis least parallel to the normal.
    let seed = if normal.x.abs() <= normal.y.abs() && normal.x.abs() <= normal.z.abs() {
        Vector3::new(1.0, 0.0, 0.0)
    } else if normal.y.abs() <= normal.z.abs() {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    };
    let u = normal
        .cross(&seed)
        .normalize()
        .unwrap_or(Vector3::new(1.0, 0.0, 0.0));
    let v = normal.cross(&u);
    (u, v)
}

/// Twice the signed area of triangle (a, b, c) — the 2D orientation test.
fn cross2(a: (f64, f64), b: (f64, f64), c: (f64, f64)) -> f64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// Do open segments `p1p2` and `p3p4` PROPERLY cross (interior intersection,
/// strictly opposite orientations on both)? Endpoint touches are not crossings.
fn segments_properly_cross(p1: (f64, f64), p2: (f64, f64), p3: (f64, f64), p4: (f64, f64)) -> bool {
    const EPS: f64 = 1e-9;
    let d1 = cross2(p3, p4, p1);
    let d2 = cross2(p3, p4, p2);
    let d3 = cross2(p1, p2, p3);
    let d4 = cross2(p1, p2, p4);
    let opposite = |a: f64, b: f64| (a > EPS && b < -EPS) || (a < -EPS && b > EPS);
    opposite(d1, d2) && opposite(d3, d4)
}

/// Does the closed polygon `p2d` (implicitly closed last→first) have any two
/// non-adjacent edges that properly cross?
#[cfg(test)]
fn polygon_has_crossing(p2d: &[(f64, f64)]) -> bool {
    !polygon_crossings(p2d, 1).is_empty()
}

/// The pairs of non-adjacent edges of the closed polygon `p2d` (implicitly
/// closed last→first) that properly cross, each edge named by its first
/// point's index; at most `most` of them.
fn polygon_crossings(p2d: &[(f64, f64)], most: usize) -> Vec<(usize, usize)> {
    let m = p2d.len();
    let mut found = Vec::new();
    for i in 0..m {
        let a1 = p2d[i];
        let a2 = p2d[(i + 1) % m];
        for j in (i + 1)..m {
            // Skip adjacent edges (share a vertex), including the wrap pair.
            if j == i + 1 || (i == 0 && j == m - 1) {
                continue;
            }
            let b1 = p2d[j];
            let b2 = p2d[(j + 1) % m];
            if segments_properly_cross(a1, a2, b1, b2) {
                found.push((i, j));
                if found.len() >= most {
                    return found;
                }
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn convex_quad_is_simple() {
        let quad = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)];
        assert!(!polygon_has_crossing(&quad));
    }

    #[test]
    fn concave_l_is_simple() {
        // An L-shape (concave but not self-intersecting).
        let l = [
            (0.0, 0.0),
            (2.0, 0.0),
            (2.0, 1.0),
            (1.0, 1.0),
            (1.0, 2.0),
            (0.0, 2.0),
        ];
        assert!(!polygon_has_crossing(&l));
    }

    #[test]
    fn bowtie_self_intersects() {
        // Classic self-crossing bow-tie.
        let bowtie = [(0.0, 0.0), (1.0, 1.0), (1.0, 0.0), (0.0, 1.0)];
        assert!(polygon_has_crossing(&bowtie));
    }

    #[test]
    fn arc_bulging_past_an_edge_self_intersects() {
        // Mimics #70: a near-quarter-circle arc whose tip overshoots the closing
        // horizontal edge, so the arc crosses it.
        let mut poly = Vec::new();
        for k in 0..=8 {
            let a = std::f64::consts::FRAC_PI_2 * (k as f64 / 8.0);
            poly.push((-0.5 * a.sin(), 0.5 * (1.0 - a.cos()) + 0.0));
        }
        // Arc tip is near (-0.5, 0.5); close back across a low horizontal edge at
        // y = 0.1 that the arc overshoots.
        poly.push((0.0, 0.1));
        poly.push((-2.0, 0.1));
        poly.push((-2.0, -1.0));
        poly.push((0.0, -1.0));
        assert!(polygon_has_crossing(&poly));
    }
}
