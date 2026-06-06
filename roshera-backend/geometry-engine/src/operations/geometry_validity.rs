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

/// True when the planar `face`'s outer loop, projected into its best-fit plane
/// with curved edges densified, is NOT a simple polygon.
fn planar_face_self_intersects(model: &BRepModel, fid: FaceId) -> bool {
    let Some(face) = model.faces.get(fid) else {
        return false;
    };
    if !is_planar_face(model, face) {
        return false;
    }
    let Some(lp) = model.loops.get(face.outer_loop) else {
        return false;
    };

    // Walk the loop in order, sampling each edge in its loop-traversal direction.
    // Straight edges contribute their start point only; curved edges contribute
    // CURVED_SAMPLES points spanning [0, 1) so the closing segment to the next
    // edge's start completes the polygon without a duplicate vertex.
    let mut pts: Vec<Point3> = Vec::with_capacity(lp.edges.len() * 2);
    for (i, &eid) in lp.edges.iter().enumerate() {
        let fwd = lp.orientations.get(i).copied().unwrap_or(true);
        let Some(edge) = model.edges.get(eid) else {
            continue;
        };
        let n = if is_line_edge(model, edge) {
            1
        } else {
            CURVED_SAMPLES
        };
        for j in 0..n {
            let s = j as f64 / n as f64;
            let t = if fwd { s } else { 1.0 - s };
            if let Ok(p) = edge.evaluate(t, &model.curves) {
                pts.push(p);
            }
        }
    }
    let m = pts.len();
    if m < 4 {
        return false; // a triangle (or less) cannot self-intersect
    }

    let Some(normal) = newell_normal(&pts) else {
        return false; // degenerate / collinear — nothing to project
    };
    let (u_axis, v_axis) = plane_axes(normal);
    let origin = pts[0];
    let p2d: Vec<(f64, f64)> = pts
        .iter()
        .map(|p| {
            let r = *p - origin;
            (r.dot(&u_axis), r.dot(&v_axis))
        })
        .collect();

    polygon_has_crossing(&p2d)
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
fn polygon_has_crossing(p2d: &[(f64, f64)]) -> bool {
    let m = p2d.len();
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
                return true;
            }
        }
    }
    false
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
