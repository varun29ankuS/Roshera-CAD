//! Mesh self-intersection oracle (PILLAR 1 certificate gap + PILLAR 2 invariant).
//!
//! A B-Rep can be topologically valid AND watertight yet be geometrically
//! SELF-OVERLAPPING — two faces that aren't topological neighbours pass through
//! each other (e.g. a chamfer cut across an existing fillet, #70, or a loft that
//! folds back on itself). No other kernel check catches this. Here we detect it
//! on the tessellated mesh: any pair of triangles that do NOT share a (welded)
//! vertex and whose interiors cross is a self-intersection.
//!
//! The cross test is segment-vs-triangle (Möller–Trumbore) over all six edges of
//! the two triangles — two surface triangles intersect iff an edge of one
//! pierces the interior of the other. Strict interior tolerances exclude the
//! legitimate edge/vertex contact of adjacent faces (which we also skip by
//! shared-vertex pruning), so a clean watertight solid reports `false`.

use crate::math::{Point3, Vector3};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};
use std::collections::HashMap;

const EPS: f64 = 1.0e-9;

/// Does the segment `p0→p1` pierce the INTERIOR of triangle `abc`? Strictly
/// interior in both the barycentric coords and the segment parameter, so shared
/// edges/vertices (touch, not cross) return `false`.
fn segment_pierces_triangle(p0: Point3, p1: Point3, a: Point3, b: Point3, c: Point3) -> bool {
    let dir: Vector3 = p1 - p0;
    let e1: Vector3 = b - a;
    let e2: Vector3 = c - a;
    let pvec = dir.cross(&e2);
    let det = e1.dot(&pvec);
    if det.abs() < EPS {
        return false; // segment parallel to the triangle plane
    }
    let inv = 1.0 / det;
    let tvec: Vector3 = p0 - a;
    let u = tvec.dot(&pvec) * inv;
    if u <= EPS || u >= 1.0 - EPS {
        return false;
    }
    let qvec = tvec.cross(&e1);
    let v = dir.dot(&qvec) * inv;
    if v <= EPS || u + v >= 1.0 - EPS {
        return false;
    }
    let t = e2.dot(&qvec) * inv;
    // Strictly inside the segment → a genuine crossing, not an endpoint touch.
    t > EPS && t < 1.0 - EPS
}

/// Do two triangles cross each other's interior? (six segment-triangle tests).
pub fn triangles_intersect(a: [Point3; 3], b: [Point3; 3]) -> bool {
    for k in 0..3 {
        let (p0, p1) = (a[k], a[(k + 1) % 3]);
        if segment_pierces_triangle(p0, p1, b[0], b[1], b[2]) {
            return true;
        }
    }
    for k in 0..3 {
        let (p0, p1) = (b[k], b[(k + 1) % 3]);
        if segment_pierces_triangle(p0, p1, a[0], a[1], a[2]) {
            return true;
        }
    }
    false
}

/// Axis-aligned bounds of a triangle (for broad-phase pruning).
fn tri_aabb(t: &[Point3; 3]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in t {
        let c = [p.x, p.y, p.z];
        for i in 0..3 {
            lo[i] = lo[i].min(c[i]);
            hi[i] = hi[i].max(c[i]);
        }
    }
    (lo, hi)
}
fn aabb_disjoint(a: &([f64; 3], [f64; 3]), b: &([f64; 3], [f64; 3])) -> bool {
    for i in 0..3 {
        if a.1[i] < b.0[i] - EPS || b.1[i] < a.0[i] - EPS {
            return true;
        }
    }
    false
}

/// `true` if the solid's tessellated mesh self-intersects at chord `chord`.
/// Pairs sharing a welded vertex (topological neighbours) are skipped; AABB
/// overlap prunes the O(n²) scan. Use a COARSE chord for a fast certificate
/// check — self-overlap is a gross geometric fault, visible at low density.
pub fn mesh_self_intersects(model: &BRepModel, solid: SolidId, chord: f64) -> bool {
    let solid_ref = match model.solids.get(solid) {
        Some(s) => s,
        None => return false,
    };
    let mut params = TessellationParams::default();
    params.chord_tolerance = chord;
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.len() < 2 {
        return false;
    }

    // Weld vertices by quantised position so adjacent triangles share canonical
    // indices (and are skipped — they touch, not cross).
    const Q: f64 = 1.0e5;
    let key = |p: &Point3| -> (i64, i64, i64) {
        (
            (p.x * Q).round() as i64,
            (p.y * Q).round() as i64,
            (p.z * Q).round() as i64,
        )
    };
    let mut canon: HashMap<(i64, i64, i64), u32> = HashMap::new();
    let mut next = 0u32;
    let welded: Vec<u32> = mesh
        .vertices
        .iter()
        .map(|v| {
            *canon.entry(key(&v.position)).or_insert_with(|| {
                let id = next;
                next += 1;
                id
            })
        })
        .collect();

    let tris: Vec<[Point3; 3]> = mesh
        .triangles
        .iter()
        .map(|t| {
            [
                mesh.vertices[t[0] as usize].position,
                mesh.vertices[t[1] as usize].position,
                mesh.vertices[t[2] as usize].position,
            ]
        })
        .collect();
    let wtri: Vec<[u32; 3]> = mesh
        .triangles
        .iter()
        .map(|t| {
            [
                welded[t[0] as usize],
                welded[t[1] as usize],
                welded[t[2] as usize],
            ]
        })
        .collect();
    let aabbs: Vec<([f64; 3], [f64; 3])> = tris.iter().map(tri_aabb).collect();

    let n = tris.len();
    for i in 0..n {
        for j in (i + 1)..n {
            // Skip topological neighbours (any shared welded vertex).
            let wi = wtri[i];
            let wj = wtri[j];
            if wi.iter().any(|x| wj.contains(x)) {
                continue;
            }
            if aabb_disjoint(&aabbs[i], &aabbs[j]) {
                continue;
            }
            if triangles_intersect(tris[i], tris[j]) {
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
    fn detects_crossing_triangles_and_clears_disjoint() {
        // Two triangles that pierce each other (an X in 3D).
        let a = [
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, 1.0),
        ];
        let b = [
            Point3::new(0.0, -1.0, 0.3),
            Point3::new(0.0, 1.0, 0.3),
            Point3::new(0.0, 0.0, -1.0),
        ];
        assert!(
            triangles_intersect(a, b),
            "crossing triangles must intersect"
        );

        // Disjoint (far apart) → no intersection.
        let c = [
            Point3::new(10.0, 10.0, 10.0),
            Point3::new(11.0, 10.0, 10.0),
            Point3::new(10.0, 11.0, 10.0),
        ];
        assert!(
            !triangles_intersect(a, c),
            "far triangles must not intersect"
        );

        // Sharing an edge (touch, not cross) → no interior crossing.
        let d = [
            Point3::new(-1.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(0.0, 0.0, -1.0),
        ];
        assert!(
            !triangles_intersect(a, d),
            "edge-sharing triangles do not self-intersect"
        );
    }
}
