//! B-Rep → polyhedral-cone bridge for contact determination (CD).
//!
//! This is where the pure cone algebra in [`crate::math::polyhedral_cone`]
//! finally touches a real solid. It builds the *first-order directional
//! structure* of a solid's boundary — the **normal cone** and **tangent cone**
//! at a vertex or along an edge — from the **exact** outward surface normals of
//! the faces meeting there (Crozet, *Smooth-BRep Contact Determination*, Ch. 3).
//!
//! The cone is exact, not tessellated: finitely many faces meet at a boundary
//! feature, and each contributes exactly one outward normal read straight off
//! its supporting surface (`Surface::normal_at`, exact even for NURBS). So the
//! "polyhedral cone" is an *exact* description of where the boundary can be
//! touched — the polyhedral structure is in the directional algebra, never in a
//! lossy approximation of the geometry.
//!
//! On top of the cones sit the two CD primitives the LMD search needs:
//!
//! * [`is_lmd_critical_direction`] — the critical-point gate: a separation
//!   direction is admissible only if it is an outward normal of *both* features
//!   (Crozet Eq. 1.23).
//! * [`features_can_contact`] — feature-pair culling: two features can produce a
//!   contact at all iff one's normal cone meets the *reflection* of the other's.
//!
//! Everything here is read-only; the model is interrogated, never mutated.

use crate::math::polyhedral_cone::{ConeIntersectionResult, PolyhedralCone};
use crate::math::vector3::{Point3, Vector3};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;
use crate::queries::lmd::face_lmds;

/// Outward unit normal of `face_id` at the 3D point `p` lying on (or nearest to)
/// it.
///
/// Reads the supporting surface's normal at the parameter closest to `p` and
/// flips it when the face is oriented `Backward` relative to its surface, so the
/// result always points *out of the solid*. Exact for analytic surfaces and for
/// NURBS (no tessellation). Returns `None` if the face or its surface is missing
/// or the surface cannot produce a unit normal there.
pub fn face_outward_normal_at(model: &BRepModel, face_id: FaceId, p: &Point3) -> Option<Vector3> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let (u, v) = surface.closest_point(p, model.tolerance()).ok()?;
    let n = surface.normal_at(u, v).ok()?.normalize().ok()?;
    // `orientation.sign()` is +1 for Forward (surface normal already outward),
    // −1 for Backward (surface normal points into the solid and must flip).
    Some(n * face.orientation.sign())
}

/// The **normal cone** of a vertex: the conic hull of the outward normals of the
/// faces meeting at it — the exact first-order directional structure of the
/// corner. A direction lies in this cone iff it is an outward normal of the
/// boundary at that vertex.
///
/// Returns `None` if the vertex or solid is unknown, or no incident face yields
/// a usable normal.
pub fn vertex_normal_cone(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> Option<PolyhedralCone> {
    let p = vertex_point(model, vertex_id)?;
    let mut normals = Vec::new();
    for face_id in solid_face_ids(model, solid_id) {
        if face_touches_vertex(model, face_id, vertex_id) {
            if let Some(n) = face_outward_normal_at(model, face_id, &p) {
                normals.push(n);
            }
        }
    }
    if normals.is_empty() {
        return None;
    }
    Some(PolyhedralCone::from_generators(&normals))
}

/// The **tangent cone** of a vertex: the polar of its normal cone — the set of
/// directions one can move while staying inside the solid to first order. For a
/// convex corner this is the intersection of the inward half-spaces of the
/// incident faces.
pub fn vertex_tangent_cone(
    model: &BRepModel,
    solid_id: SolidId,
    vertex_id: VertexId,
) -> Option<PolyhedralCone> {
    Some(vertex_normal_cone(model, solid_id, vertex_id)?.polar())
}

/// The **normal cone** along an edge: the conic hull of the outward normals of
/// the faces sharing it, evaluated at the edge's mid-point. For a smooth (G1)
/// edge the two normals coincide and this collapses to a single ray; for a
/// convex crease it is the dihedral wedge between the two face normals.
///
/// Returns `None` if the edge or solid is unknown, or no incident face yields a
/// usable normal.
pub fn edge_normal_cone(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> Option<PolyhedralCone> {
    let mid = edge_midpoint(model, edge_id)?;
    let mut normals = Vec::new();
    for face_id in solid_face_ids(model, solid_id) {
        if face_uses_edge(model, face_id, edge_id) {
            if let Some(n) = face_outward_normal_at(model, face_id, &mid) {
                normals.push(n);
            }
        }
    }
    if normals.is_empty() {
        return None;
    }
    Some(PolyhedralCone::from_generators(&normals))
}

/// The **tangent cone** along an edge: the polar of its normal cone.
pub fn edge_tangent_cone(
    model: &BRepModel,
    solid_id: SolidId,
    edge_id: EdgeId,
) -> Option<PolyhedralCone> {
    Some(edge_normal_cone(model, solid_id, edge_id)?.polar())
}

/// The **critical-point gate** (Crozet Eq. 1.23).
///
/// `d` is a candidate unit separation direction pointing *from* feature A *to*
/// feature B. The footpoint pair can be a local minimum-distance critical point
/// only if `d` is an outward normal of A (so A's boundary recedes from B along
/// `d`) and `-d` is an outward normal of B. Equivalently: `d ∈ N_A` and
/// `-d ∈ N_B`, where `N_•` are the (possibly dilated) normal cones.
///
/// For curved features pass cones already widened with
/// [`PolyhedralCone::dilate`] so the single-direction test conservatively covers
/// the whole patch.
pub fn is_lmd_critical_direction(
    d: &Vector3,
    normal_cone_a: &PolyhedralCone,
    normal_cone_b: &PolyhedralCone,
) -> bool {
    normal_cone_a.contains(d) && normal_cone_b.contains(&(-*d))
}

/// **Feature-pair culling.** Can features A and B touch at all, given only their
/// normal cones? A contact needs a direction `d` with `d ∈ N_A` and `-d ∈ N_B`;
/// such a `d` exists iff `N_A` meets the reflected cone `-N_B`. When this returns
/// `false` the pair can be discarded before any (expensive) LMD search.
pub fn features_can_contact(
    normal_cone_a: &PolyhedralCone,
    normal_cone_b: &PolyhedralCone,
) -> bool {
    matches!(
        normal_cone_a.intersects(&normal_cone_b.negated()),
        ConeIntersectionResult::Overlapping
    )
}

// ---------------------------------------------------------------------------
// Topology walks (private)
// ---------------------------------------------------------------------------

/// 3D position of a vertex as a [`Point3`].
fn vertex_point(model: &BRepModel, vertex_id: VertexId) -> Option<Point3> {
    let v = model.vertices.get(vertex_id)?;
    Some(Vector3::new(v.position[0], v.position[1], v.position[2]))
}

/// Mid-point of an edge — the curve evaluated at its mid-parameter when the
/// curve is available, else the average of the two endpoint vertices (exact for
/// the straight-edge case).
fn edge_midpoint(model: &BRepModel, edge_id: EdgeId) -> Option<Point3> {
    let edge = model.edges.get(edge_id)?;
    let t_mid = 0.5 * (edge.param_range.start + edge.param_range.end);
    if let Some(curve) = model.curves.get(edge.curve_id) {
        if let Ok(p) = curve.point_at(t_mid) {
            return Some(p);
        }
    }
    let a = vertex_point(model, edge.start_vertex)?;
    let b = vertex_point(model, edge.end_vertex)?;
    Some((a + b) * 0.5)
}

/// All face ids in a solid (outer shell plus any void shells).
fn solid_face_ids(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return out;
    };
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend(solid.inner_shells.iter().copied());
    for sid in shell_ids {
        if let Some(shell) = model.shells.get(sid) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

/// Does any boundary loop of `face_id` reference `vertex_id`?
fn face_touches_vertex(model: &BRepModel, face_id: FaceId, vertex_id: VertexId) -> bool {
    face_edge_ids(model, face_id)
        .into_iter()
        .any(|eid| match model.edges.get(eid) {
            Some(edge) => edge.start_vertex == vertex_id || edge.end_vertex == vertex_id,
            None => false,
        })
}

/// Does any boundary loop of `face_id` contain `edge_id`?
fn face_uses_edge(model: &BRepModel, face_id: FaceId, edge_id: EdgeId) -> bool {
    face_edge_ids(model, face_id).contains(&edge_id)
}

/// Edge ids across a face's outer and inner loops.
fn face_edge_ids(model: &BRepModel, face_id: FaceId) -> Vec<EdgeId> {
    let mut out = Vec::new();
    let Some(face) = model.faces.get(face_id) else {
        return out;
    };
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        if let Some(lp) = model.loops.get(lid) {
            out.extend(lp.edges.iter().copied());
        }
    }
    out
}

// ===========================================================================
// Contact-manifold extraction — the narrow-phase output a physics engine reads.
//
// The LMD layer ([`crate::queries::lmd`]) and the feature culling above answer
// "what is the closest approach between two boundary features." A rigid-body
// solver (parry/rapier, #41/#42) needs more than a scalar: for every active
// contact it needs the *point on each solid*, the *unit contact normal*, and a
// *signed* gap (negative = interpenetrating, magnitude = penetration depth).
// That is exactly parry's `Contact { point1, point2, normal1, dist }`, so this
// module is the seam the roshera-parry `QueryDispatcher` will hand to parry.
//
// Candidate generation reuses the *same* face-LMD + edge-edge enumeration the
// CD ablation oracle validates against brute force, so the reported minimum is
// the true minimum. The only additions are (a) keeping the contact *points and
// normals*, not just the distance, and (b) a signed-distance / penetration-depth
// pass driven by the convex interior-overlap test, so penetration is reported as
// a negative gap along a real separation axis rather than a positive nearest-
// feature distance. Read-only throughout.
// ===========================================================================

/// A single contact between two solids: the witness point on each solid's
/// boundary, the unit contact normal, and the SIGNED gap.
///
/// `normal` is the unit separation direction measured *on solid A*: it points
/// out of A toward B — the direction B must travel to break contact. This is
/// parry's `Contact::normal1`; the normal on B is simply `-normal`. `distance`
/// is positive when the solids are apart (the witnesses are `distance` apart),
/// zero at grazing contact, and **negative when the solids interpenetrate**, its
/// magnitude being the penetration depth along `normal`.
#[derive(Debug, Clone, Copy)]
pub struct Contact {
    pub point_a: Point3,
    pub point_b: Point3,
    pub normal: Vector3,
    pub distance: f64,
}

/// A contact manifold between two solids: every contact within the `prediction`
/// margin, most-penetrating / nearest first, plus whether the interiors overlap.
/// A solver consumes `points` as the contact set and `penetrating` as the sign
/// of the principal gap.
#[derive(Debug, Clone)]
pub struct ContactManifold {
    pub points: Vec<Contact>,
    pub penetrating: bool,
}

#[inline]
fn proj(p: Point3, d: &Vector3) -> f64 {
    p.x * d.x + p.y * d.y + p.z * d.z
}

#[inline]
fn unit(v: Vector3) -> Option<Vector3> {
    let m = v.magnitude();
    if m > 1e-12 {
        Some(v * (1.0 / m))
    } else {
        None
    }
}

/// Sample an edge's carrier curve into `n` segments over its parameter sub-range
/// (line edges land their two true endpoints; curved edges are densified).
fn sample_edge_pts(model: &BRepModel, edge_id: EdgeId, n: usize) -> Vec<Point3> {
    let mut pts = Vec::new();
    let Some(edge) = model.edges.get(edge_id) else {
        return pts;
    };
    let Some(curve) = model.curves.get(edge.curve_id) else {
        return pts;
    };
    let (s, e) = (edge.param_range.start, edge.param_range.end);
    for k in 0..=n {
        let t = s + (e - s) * (k as f64) / (n as f64);
        if let Ok(p) = curve.point_at(t) {
            pts.push(p);
        }
    }
    pts
}

/// Closest points between two 3D segments (Ericson, *Real-Time Collision
/// Detection* §5.1.9): returns `(distance, witness on [p1,q1], witness on
/// [p2,q2])`. Exact for straight edges; the polyline approximation for curved
/// edges is within contact tolerance at the sample density used.
fn seg_seg_closest(p1: Point3, q1: Point3, p2: Point3, q2: Point3) -> (f64, Point3, Point3) {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(&d1);
    let e = d2.dot(&d2);
    let f = d2.dot(&r);
    let eps = 1e-18;
    let (s, t);
    if a <= eps && e <= eps {
        return (r.magnitude(), p1, p2);
    }
    if a <= eps {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(&r);
        if e <= eps {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(&d2);
            let denom = a * e - b * b;
            let s0 = if denom.abs() > eps {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                t = t0;
                s = s0;
            }
        }
    }
    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    ((c1 - c2).magnitude(), c1, c2)
}

/// Nearest approach between two B-Rep edges with witness points, or `None` if an
/// edge degenerates below two samples.
fn edge_pair_contact(model: &BRepModel, ea: EdgeId, eb: EdgeId) -> Option<(f64, Point3, Point3)> {
    const N: usize = 4;
    let pa = sample_edge_pts(model, ea, N);
    let pb = sample_edge_pts(model, eb, N);
    if pa.len() < 2 || pb.len() < 2 {
        return None;
    }
    let mut best: Option<(f64, Point3, Point3)> = None;
    for i in 0..pa.len() - 1 {
        for j in 0..pb.len() - 1 {
            let (d, c1, c2) = seg_seg_closest(pa[i], pa[i + 1], pb[j], pb[j + 1]);
            if best.map_or(true, |(bd, _, _)| d < bd) {
                best = Some((d, c1, c2));
            }
        }
    }
    best
}

/// Support value of a CONVEX solid along `dir`: `max over the boundary of p·dir`.
/// Analytic for the canonical curved faces (sphere centre + radius; cylinder rim
/// circles) and the boundary-edge sample cloud otherwise — together exact for
/// boxes (line edges hit the true vertices) and the canonical curved primitives,
/// a tight bound for cones (base-rim circle sampled). Used only to size a
/// penetration depth along a known axis.
fn support_max(model: &BRepModel, solid: SolidId, dir: &Vector3) -> f64 {
    use crate::primitives::surface::{Cylinder, Sphere};
    let mut m = f64::NEG_INFINITY;
    for fid in solid_face_ids(model, solid) {
        if let Some(face) = model.faces.get(fid) {
            if let Some(surf) = model.surfaces.get(face.surface_id) {
                if let Some(s) = surf.as_any().downcast_ref::<Sphere>() {
                    m = m.max(proj(s.center, dir) + s.radius);
                } else if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
                    if let Some([h0, h1]) = c.height_limits {
                        let along = dir.dot(&c.axis);
                        let perp = (1.0 - along * along).max(0.0).sqrt();
                        for h in [h0, h1] {
                            let end = c.origin + c.axis * h;
                            m = m.max(proj(end, dir) + c.radius * perp);
                        }
                    }
                }
            }
        }
        for eid in face_edge_ids(model, fid) {
            for p in sample_edge_pts(model, eid, 4) {
                m = m.max(proj(p, dir));
            }
        }
    }
    m
}

/// `min over the boundary of p·dir` = `-support_max(-dir)`.
fn support_min(model: &BRepModel, solid: SolidId, dir: &Vector3) -> f64 {
    -support_max(model, solid, &(*dir * -1.0))
}

/// A point guaranteed inside a CONVEX solid: the analytic centre of a canonical
/// curved face (sphere centre / cylinder axis-midpoint) — robust where a curved
/// solid carries no usable boundary vertex — else the boundary-vertex average
/// (their hull ⊆ the solid).
fn interior_point(model: &BRepModel, solid: SolidId) -> Option<Point3> {
    use crate::primitives::surface::{Cylinder, Sphere};
    use std::collections::HashSet;

    for fid in solid_face_ids(model, solid) {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let Some(surf) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(s) = surf.as_any().downcast_ref::<Sphere>() {
            return Some(s.center);
        }
        if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
            if let Some([h0, h1]) = c.height_limits {
                return Some(c.origin + c.axis * (0.5 * (h0 + h1)));
            }
        }
    }

    let mut seen: HashSet<u32> = HashSet::new();
    let (mut sx, mut sy, mut sz, mut n) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    for fid in solid_face_ids(model, solid) {
        for eid in face_edge_ids(model, fid) {
            if let Some(e) = model.edges.get(eid) {
                for vid in [e.start_vertex, e.end_vertex] {
                    if seen.insert(vid) {
                        if let Some(p) = model.vertices.get_position(vid) {
                            sx += p[0];
                            sy += p[1];
                            sz += p[2];
                            n += 1.0;
                        }
                    }
                }
            }
        }
    }
    if n == 0.0 {
        return None;
    }
    Some(Point3::new(sx / n, sy / n, sz / n))
}

/// Convex point-in-solid: inside iff on the inner side of every bounding face
/// (signed distance along the OUTWARD-oriented surface normal ≤ tol). The normal
/// is oriented away from `interior`, so this is independent of stored face
/// orientation and robust for curved convex solids where a winding-number shell
/// test under-resolves a seam-bounded face's solid angle.
fn point_in_convex_solid(model: &BRepModel, solid: SolidId, interior: &Point3, p: &Point3) -> bool {
    let tol = crate::math::Tolerance::default();
    let tol_d = tol.distance();
    let mut tested = false;
    for fid in solid_face_ids(model, solid) {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let Some(surf) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        let Ok((u, v)) = surf.closest_point(p, tol) else {
            continue;
        };
        let Ok(eval) = surf.evaluate_full(u, v) else {
            continue;
        };
        let sp = eval.position;
        let outward = if eval.normal.dot(&(sp - *interior)) < 0.0 {
            eval.normal * -1.0
        } else {
            eval.normal
        };
        tested = true;
        if (*p - sp).dot(&outward) > tol_d {
            return false;
        }
    }
    tested
}

/// Do the two solids' interiors overlap? Sample the segment between an interior
/// point of each (threading any convex overlap lens) and report true if a sample
/// lies inside both closed shells.
fn interiors_overlap(model: &BRepModel, a: SolidId, b: SolidId, ia: &Point3, ib: &Point3) -> bool {
    for k in 0..=4 {
        let t = k as f64 / 4.0;
        let p = Point3::new(
            ia.x + (ib.x - ia.x) * t,
            ia.y + (ib.y - ia.y) * t,
            ia.z + (ib.z - ia.z) * t,
        );
        if point_in_convex_solid(model, a, ia, &p) && point_in_convex_solid(model, b, ib, &p) {
            return true;
        }
    }
    false
}

/// The full contact manifold between two solids within `prediction`.
///
/// Enumerates every face-pair LMD and edge-edge nearest approach (the candidate
/// set the CD ablation oracle proves complete), keeps the witnesses and normals,
/// dedups near-coincident contacts, and orders them nearest-first. When the
/// interiors overlap, the manifold reports a single shared separation axis
/// (interior-to-interior) with a **negative** signed distance equal to the
/// directional penetration depth (`support_max(A) − support_min(B)` along that
/// axis) — the sign and depth a position-based solver needs. When the solids are
/// apart, each contact carries its own A→B normal and its positive gap, and any
/// candidate beyond `prediction` is dropped.
pub fn solid_contact_manifold(
    model: &BRepModel,
    a: SolidId,
    b: SolidId,
    prediction: f64,
) -> ContactManifold {
    let faces_a = solid_face_ids(model, a);
    let faces_b = solid_face_ids(model, b);

    // Candidate witnesses: (distance, point on A, point on B).
    let mut cands: Vec<(f64, Point3, Point3)> = Vec::new();
    for &fa in &faces_a {
        let ea_ids = face_edge_ids(model, fa);
        for &fb in &faces_b {
            for lmd in face_lmds(model, fa, fb) {
                cands.push((lmd.distance, lmd.point_a, lmd.point_b));
            }
            for &ea in &ea_ids {
                for &eb in &face_edge_ids(model, fb) {
                    if let Some((d, pa, pb)) = edge_pair_contact(model, ea, eb) {
                        cands.push((d, pa, pb));
                    }
                }
            }
        }
    }
    cands.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));

    let ia = interior_point(model, a);
    let ib = interior_point(model, b);
    let penetrating = match (ia, ib) {
        (Some(pa), Some(pb)) => interiors_overlap(model, a, b, &pa, &pb),
        _ => false,
    };

    // Penetration axis + depth (only meaningful when overlapping).
    let pen_axis = match (ia, ib) {
        (Some(pa), Some(pb)) => unit(pb - pa),
        _ => None,
    }
    .unwrap_or(Vector3::Z);
    let pen_depth = if penetrating {
        (support_max(model, a, &pen_axis) - support_min(model, b, &pen_axis)).max(0.0)
    } else {
        0.0
    };

    const MERGE_TOL: f64 = 1e-4;
    let mut points: Vec<Contact> = Vec::new();
    for (d, pa, pb) in cands {
        // When apart, honour the prediction margin. When penetrating, always
        // admit at least the principal (nearest-feature) contact.
        if !penetrating && d > prediction {
            continue;
        }
        if penetrating && d > prediction && !points.is_empty() {
            continue;
        }
        if points.iter().any(|c| {
            (c.point_a - pa).magnitude() < MERGE_TOL && (c.point_b - pb).magnitude() < MERGE_TOL
        }) {
            continue;
        }
        let (normal, distance) = if penetrating {
            (pen_axis, -pen_depth)
        } else {
            let n = unit(pb - pa)
                .or_else(|| match (ia, ib) {
                    (Some(p), Some(q)) => unit(q - p),
                    _ => None,
                })
                .unwrap_or(Vector3::Z);
            (n, d)
        };
        points.push(Contact {
            point_a: pa,
            point_b: pb,
            normal,
            distance,
        });
    }

    ContactManifold {
        points,
        penetrating,
    }
}

/// The single principal contact between two solids within `prediction`: the
/// most-penetrating (or nearest) witness pair, normal, and signed gap — the
/// minimal datum parry's `QueryDispatcher::contact` returns. `None` when the
/// solids are farther apart than `prediction` and not interpenetrating.
pub fn solid_contact(
    model: &BRepModel,
    a: SolidId,
    b: SolidId,
    prediction: f64,
) -> Option<Contact> {
    solid_contact_manifold(model, a, b, prediction)
        .points
        .into_iter()
        .next()
}

/// Boolean intersection test: do the two solids' interiors overlap? This is the
/// `QueryDispatcher::intersection_test` half of the parry surface (the cheap
/// yes/no a broad phase confirms before asking for a full contact manifold). It
/// is the convex interior-overlap probe, so it answers true for genuine volume
/// overlap (penetration / containment) and false for mere surface grazing.
pub fn solids_intersect(model: &BRepModel, a: SolidId, b: SolidId) -> bool {
    match (interior_point(model, a), interior_point(model, b)) {
        (Some(pa), Some(pb)) => interiors_overlap(model, a, b, &pa, &pb),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const X: Vector3 = Vector3::X;
    const Y: Vector3 = Vector3::Y;
    const Z: Vector3 = Vector3::Z;

    /// Build a 2×2×2 box centred at the origin (corners at ±1) and return the
    /// model plus its solid id.
    fn unit_box() -> (BRepModel, SolidId) {
        use crate::primitives::topology_builder::TopologyBuilder;
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box creation succeeds");
        let solid_id = model
            .solids
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("box has a solid");
        (model, solid_id)
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    /// The vertex id whose position is closest to `target`.
    fn vertex_at(model: &BRepModel, target: Vector3) -> VertexId {
        model
            .vertices
            .iter()
            .min_by(|(_, va), (_, vb)| {
                let da = (Vector3::new(va.position[0], va.position[1], va.position[2]) - target)
                    .magnitude();
                let db = (Vector3::new(vb.position[0], vb.position[1], vb.position[2]) - target)
                    .magnitude();
                da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(id, _)| id)
            .expect("box has vertices")
    }

    /// An edge id whose two endpoints are closest to `a` and `b` (either order).
    fn edge_between(model: &BRepModel, a: Vector3, b: Vector3) -> EdgeId {
        let va = vertex_at(model, a);
        let vb = vertex_at(model, b);
        model
            .edges
            .iter()
            .find(|(_, e)| {
                (e.start_vertex == va && e.end_vertex == vb)
                    || (e.start_vertex == vb && e.end_vertex == va)
            })
            .map(|(id, _)| id)
            .expect("edge between the two corners exists")
    }

    fn ray(g: Vector3) -> PolyhedralCone {
        PolyhedralCone::from_generators(&[g])
    }

    // -- vertex normal cone ------------------------------------------------

    #[test]
    fn corner_normal_cone_is_the_positive_octant() {
        let (model, solid) = unit_box();
        let v = vertex_at(&model, Vector3::new(1.0, 1.0, 1.0));
        let cone = vertex_normal_cone(&model, solid, v).expect("corner has a normal cone");

        // Three faces meet → a pointed rank-3 cone: 3 generators, 3 supports.
        assert_eq!(cone.generators().len(), 3, "three incident faces");
        assert_eq!(cone.supports().len(), 3, "pointed cone has three supports");

        // It is the (+,+,+) octant: contains the three axes and the diagonal,
        // excludes the opposite directions.
        assert!(cone.contains(&X) && cone.contains(&Y) && cone.contains(&Z));
        assert!(cone.contains(&Vector3::new(1.0, 1.0, 1.0)));
        assert!(!cone.contains(&(-X)) && !cone.contains(&(-Y)) && !cone.contains(&(-Z)));
        assert!(!cone.contains(&Vector3::new(-1.0, -1.0, -1.0)));
    }

    #[test]
    fn every_corner_points_its_normal_cone_outward() {
        let (model, solid) = unit_box();
        // All eight sign combinations of (±1,±1,±1).
        for &sx in &[-1.0_f64, 1.0] {
            for &sy in &[-1.0_f64, 1.0] {
                for &sz in &[-1.0_f64, 1.0] {
                    let corner = Vector3::new(sx, sy, sz);
                    let v = vertex_at(&model, corner);
                    let cone =
                        vertex_normal_cone(&model, solid, v).expect("corner normal cone exists");
                    assert_eq!(cone.generators().len(), 3);
                    assert_eq!(cone.supports().len(), 3);
                    // Normal cone points away from centre: contains the outward
                    // diagonal, not the inward one.
                    let outward = corner; // centre is the origin
                    assert!(
                        cone.contains(&outward),
                        "normal cone must contain the outward diagonal at {corner:?}"
                    );
                    assert!(
                        !cone.contains(&(-outward)),
                        "normal cone must exclude the inward diagonal at {corner:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn corner_tangent_cone_is_the_polar_octant() {
        let (model, solid) = unit_box();
        let corner = Vector3::new(1.0, 1.0, 1.0);
        let v = vertex_at(&model, corner);
        let tangent = vertex_tangent_cone(&model, solid, v).expect("tangent cone exists");

        // Feasible directions from the (+,+,+) corner point back into the box:
        // the (−,−,−) octant. The direction to the box centre is admissible;
        // the outward diagonal is not.
        assert!(tangent.contains(&Vector3::new(-1.0, -1.0, -1.0)));
        assert!(!tangent.contains(&Vector3::new(1.0, 1.0, 1.0)));

        // Polarity: tangent == polar(normal), checked structurally.
        let normal = vertex_normal_cone(&model, solid, v).expect("normal cone exists");
        assert_eq!(tangent.generators().len(), normal.supports().len());
    }

    // -- edge normal cone --------------------------------------------------

    #[test]
    fn box_edge_normal_cone_is_a_two_face_wedge() {
        let (model, solid) = unit_box();
        // Vertical edge at x=+1, y=+1 (between the two z corners). Faces x=+1
        // (normal +X) and y=+1 (normal +Y) meet there.
        let edge = edge_between(
            &model,
            Vector3::new(1.0, 1.0, 1.0),
            Vector3::new(1.0, 1.0, -1.0),
        );
        let cone = edge_normal_cone(&model, solid, edge).expect("edge has a normal cone");

        assert_eq!(cone.generators().len(), 2, "two faces meet at the edge");
        // The wedge spanned by +X and +Y: contains their bisector, excludes the
        // out-of-plane axis and the opposite directions.
        assert!(cone.contains(&X) && cone.contains(&Y));
        assert!(cone.contains(&Vector3::new(1.0, 1.0, 0.0)));
        assert!(!cone.contains(&Z) && !cone.contains(&(-Z)));
        assert!(!cone.contains(&(-X)) && !cone.contains(&(-Y)));
    }

    // -- critical-point gate ----------------------------------------------

    #[test]
    fn opposed_faces_pass_the_critical_gate_along_their_normal() {
        // A's +X face vs B's −X face: separation A→B is +X.
        let a = ray(X);
        let b = ray(-X);
        assert!(is_lmd_critical_direction(&X, &a, &b));
        // Any other direction fails — it is not an outward normal of A.
        assert!(!is_lmd_critical_direction(&Y, &a, &b));
        assert!(!is_lmd_critical_direction(&(-X), &a, &b));
    }

    #[test]
    fn critical_gate_is_symmetric_under_reversal() {
        let (model, solid) = unit_box();
        let na = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(1.0, 1.0, 1.0)),
        )
        .expect("normal cone");
        let nb = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(-1.0, -1.0, -1.0)),
        )
        .expect("normal cone");
        // d admissible for (A,B) ⟺ −d admissible for (B,A).
        let d = Vector3::new(1.0, 1.0, 1.0)
            .normalize()
            .expect("nonzero direction");
        assert_eq!(
            is_lmd_critical_direction(&d, &na, &nb),
            is_lmd_critical_direction(&(-d), &nb, &na)
        );
    }

    // -- feature-pair culling ---------------------------------------------

    #[test]
    fn opposed_faces_can_contact_aligned_faces_cannot() {
        // +X face vs −X face: they face each other → contact possible.
        assert!(features_can_contact(&ray(X), &ray(-X)));
        // +X face vs +X face: both point the same way → no face-to-face contact.
        assert!(!features_can_contact(&ray(X), &ray(X)));
    }

    #[test]
    fn opposite_box_corners_can_mate_same_corner_cannot() {
        let (model, solid) = unit_box();
        let pos = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(1.0, 1.0, 1.0)),
        )
        .expect("normal cone");
        let neg = vertex_normal_cone(
            &model,
            solid,
            vertex_at(&model, Vector3::new(-1.0, -1.0, -1.0)),
        )
        .expect("normal cone");
        // The (+,+,+) corner's outward octant and the (−,−,−) corner's outward
        // octant are reflections of one another → a mating direction exists.
        assert!(features_can_contact(&pos, &neg));
        // A corner cannot mate face-to-face with a copy of itself.
        assert!(!features_can_contact(&pos, &pos));
    }

    #[test]
    fn culling_matches_explicit_gate_witness() {
        // features_can_contact is exactly "∃ d: gate(d, A, B)". Cross-check the
        // headline case: when culling says yes, the constructive direction works.
        let a = ray(X);
        let b = ray(-X);
        assert!(features_can_contact(&a, &b));
        assert!(
            is_lmd_critical_direction(&X, &a, &b),
            "the witness direction reported by the geometry passes the gate"
        );
        // And when culling says no, no axis-aligned witness exists.
        let c = ray(X);
        assert!(!features_can_contact(&a, &c));
        for d in [X, -X, Y, -Y, Z, -Z] {
            assert!(!is_lmd_critical_direction(&d, &a, &c));
        }
    }

    // -- outward-normal sanity --------------------------------------------

    #[test]
    fn face_normals_on_a_box_point_outward() {
        let (model, solid) = unit_box();
        // Sample each face by its incident corner; the normal must have a
        // positive component along the outward corner direction.
        let corner = Vector3::new(1.0, 1.0, 1.0);
        for face_id in solid_face_ids(&model, solid) {
            if face_touches_vertex(&model, face_id, vertex_at(&model, corner)) {
                let n = face_outward_normal_at(&model, face_id, &corner)
                    .expect("box face yields a normal");
                assert!(approx(n.magnitude(), 1.0), "normal is unit length");
                assert!(
                    n.dot(&corner) > 0.0,
                    "face normal at corner {corner:?} points outward (n = {n:?})"
                );
            }
        }
    }

    // -- property tests ----------------------------------------------------

    use proptest::prelude::*;

    fn unit_vec() -> impl Strategy<Value = Vector3> {
        (-1.0_f64..1.0, -1.0_f64..1.0, -1.0_f64..1.0).prop_filter_map("nonzero", |(x, y, z)| {
            Vector3::new(x, y, z).normalize().ok()
        })
    }

    proptest! {
        /// The critical-point gate is reversal-symmetric for any cones and any
        /// direction: gate(d, A, B) ⟺ gate(−d, B, A). This is the defining
        /// symmetry of a footpoint pair (swap the roles of the two solids and
        /// flip the connecting direction).
        #[test]
        fn gate_reversal_symmetry(d in unit_vec(), g1 in unit_vec(), g2 in unit_vec()) {
            let a = PolyhedralCone::from_generators(&[g1]);
            let b = PolyhedralCone::from_generators(&[g2]);
            prop_assert_eq!(
                is_lmd_critical_direction(&d, &a, &b),
                is_lmd_critical_direction(&(-d), &b, &a)
            );
        }

        /// Culling never rejects a real contact: if some direction passes the
        /// gate, `features_can_contact` must return `true`. (Soundness — culling
        /// is conservative, it only discards pairs that genuinely cannot touch.)
        #[test]
        fn culling_never_drops_a_gated_pair(d in unit_vec(), g1 in unit_vec(), g2 in unit_vec()) {
            let a = PolyhedralCone::from_generators(&[g1]);
            let b = PolyhedralCone::from_generators(&[g2]);
            if is_lmd_critical_direction(&d, &a, &b) {
                prop_assert!(features_can_contact(&a, &b));
            }
        }

        /// Culling is symmetric in its two features: a pair can contact
        /// regardless of which feature is named first.
        #[test]
        fn culling_is_symmetric(g1 in unit_vec(), g2 in unit_vec()) {
            let a = PolyhedralCone::from_generators(&[g1]);
            let b = PolyhedralCone::from_generators(&[g2]);
            prop_assert_eq!(
                features_can_contact(&a, &b),
                features_can_contact(&b, &a)
            );
        }
    }

    // Keep the `ConeIntersectionResult` import meaningful even if the matches!
    // in `features_can_contact` is the only other user.
    #[test]
    fn intersection_result_is_in_scope() {
        let overlapping = ray(X).intersects(&ray(X));
        assert!(matches!(overlapping, ConeIntersectionResult::Overlapping));
    }
}
