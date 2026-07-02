//! Deterministic mesh-level DEFECT INJECTORS — the mutation side of the
//! injected-defect benchmark (Move 2).
//!
//! Each injector takes a SOUND [`TriangleMesh`] and returns a mutated copy that
//! embodies one class of silent geometric lie a downstream consumer (viewport,
//! exporter, printer) would otherwise swallow. They are pure, deterministic (no
//! randomness), and touch ONLY the mesh — never the B-Rep — which is precisely
//! why the shallow `brep_valid`-only baseline is blind to all of them.
//!
//! The two flagship injectors ([`flip_normal`], [`inject_self_intersection`])
//! are deliberately constructed to keep the "looks-closed" undirected mesh-count
//! baseline blind too: neither changes the boundary/non-manifold edge tallies.
//! Only the certified eye's `oriented` / `self_intersection_free` dimensions
//! catch them. The other two ([`delete_triangle`], [`duplicate_triangle`]) are
//! honest controls the count baseline DOES catch.
//!
//! These consume the same mesh the certificate analyses and the renderer draw,
//! so the mutated mesh IS the artifact the VLM tier renders.

use crate::math::Point3;
use crate::tessellation::mesh::TriangleMesh;

/// Absolute distance below which two mesh vertices are the same point — matches
/// the weld epsilon the harness uses for the 1–10 unit parts this benchmark
/// builds. Used to move an entire co-located vertex group as one (preserving
/// welded connectivity) in [`inject_self_intersection`].
const WELD_EPS: f64 = 1e-6;

/// #1 FLIPPED FACE NORMAL — reverse ONE triangle's winding (`[a,b,c] → [a,c,b]`).
///
/// The undirected edge set is unchanged (so the mesh still "counts closed"), but
/// every edge shared with a neighbour is now traversed the SAME direction by both
/// triangles → a repeated *directed* edge. Only the `oriented` dimension
/// (inconsistent directed edges) flags it; watertight/manifold counts stay clean.
pub fn flip_normal(mesh: &TriangleMesh) -> TriangleMesh {
    let mut m = mesh.clone();
    if let Some(&t) = m.triangles.first() {
        if let Some(slot) = m.triangles.first_mut() {
            // Swap the last two indices → reversed winding, same vertex set.
            *slot = [t[0], t[2], t[1]];
        }
    }
    m
}

/// #2 SELF-INTERSECTION — a VERTEX-level mutation: translate the entire welded
/// group of vertices sitting at the +X extreme a full bounding-box span past the
/// −X side.
///
/// Moving a co-located group as one unit keeps EVERY connectivity count identical
/// — no edge is added, removed, or re-shared, so boundary / non-manifold /
/// directed-edge tallies are untouched and the "looks-closed" baseline stays
/// blind. Geometrically, the facets fanning off that corner now spike straight
/// through the body and pierce the opposite wall, so `self_intersection_free` is
/// the only dimension that catches it. This is the construction that keeps the
/// flagship contrast honest: there is no edge-count tell for the shallow eye.
pub fn inject_self_intersection(mesh: &TriangleMesh) -> TriangleMesh {
    let mut m = mesh.clone();
    let Some(first) = m.vertices.first() else {
        return m;
    };
    let mut lo = first.position;
    let mut hi = first.position;
    let mut anchor = first.position;
    for v in &m.vertices {
        let p = v.position;
        lo = Point3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Point3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
        if p.x > anchor.x {
            anchor = p;
        }
    }
    let span_x = (hi.x - lo.x).max(1.0);
    // A point a full span BEYOND the −X face, on the anchor's own y/z line, so the
    // spike runs the length of the solid and exits through the opposite wall.
    let target = Point3::new(lo.x - span_x, anchor.y, anchor.z);
    for v in m.vertices.iter_mut() {
        if (v.position - anchor).magnitude() < WELD_EPS {
            v.position = target;
        }
    }
    m
}

/// #3 TORN FACET (sub-mm gap / unwelded seam) — delete one triangle.
///
/// Its three edges drop to a single incident triangle → three boundary edges.
/// `watertight` catches it, and so does the undirected mesh-count baseline — an
/// honest control proving the shallow mesh-eye is not uniformly blind. The
/// per-triangle `face_map` is kept in lockstep with `triangles`.
pub fn delete_triangle(mesh: &TriangleMesh) -> TriangleMesh {
    let mut m = mesh.clone();
    if m.triangles.is_empty() {
        return m;
    }
    m.triangles.remove(0);
    if !m.face_map.is_empty() {
        m.face_map.remove(0);
    }
    m
}

/// #4 DUPLICATED FACET (non-manifold) — append a copy of an existing triangle.
///
/// Each of its edges is now bordered by three triangles → non-manifold edges.
/// `manifold` catches it, and so does the mesh-count baseline (control). The
/// `face_map` gains the matching face id so it stays aligned with `triangles`.
pub fn duplicate_triangle(mesh: &TriangleMesh) -> TriangleMesh {
    let mut m = mesh.clone();
    let Some(&t) = m.triangles.first() else {
        return m;
    };
    m.triangles.push(t);
    if let Some(&f) = m.face_map.first() {
        m.face_map.push(f);
    }
    m
}
