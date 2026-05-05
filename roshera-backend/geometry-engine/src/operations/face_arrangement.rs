//! DCEL-based planar face arrangement for boolean face splitting.
//!
//! Given an [`IntersectionGraph`] built from a face's boundary edges and the
//! cutting curves that split it, this module:
//!
//!   1. Emits two [`HalfEdge`]s per undirected graph edge (a DCEL).
//!   2. Sorts outgoing half-edges at every vertex by angle in the surface's
//!      tangent plane (CCW under the surface normal).
//!   3. Wires `next` pointers using the standard DCEL formula
//!      (next(h) = CW-previous of twin(h) in the outgoing-CCW list at
//!      twin(h).origin; de Berg §2.2).
//!   4. Walks minimal face cycles, discards dangling-edge excursions and
//!      the outer (CW-under-normal) face, returns the remaining regions.
//!
//! # Why this module exists
//!
//! The previous `extract_face_loops` walked the intersection graph by
//! `HashSet<EdgeId>` iteration order and picked the first unused incident
//! edge at each vertex — no angular sort. This could emerge with the
//! original boundary unchanged and cutting curves left dangling, or produce
//! cycles that spanned multiple regions, causing tier-3 property tests
//! (bbox containment) to fail for non-trivial box/box booleans.
//!
//! # References
//! - de Berg, van Kreveld, Overmars, Schwarzkopf (2008).
//!   *Computational Geometry: Algorithms and Applications*, §2.2
//!   (Doubly-Connected Edge Lists).
//! - Vida, Martin, Varady (1994). *A survey of blending methods that use
//!   parametric surfaces*, Comp. Aided Design 26(5) — angular arrangement
//!   at non-manifold vertices.
//! - Piegl & Tiller (1997). *The NURBS Book*, §17 (trimmed-surface face
//!   construction).
//!
//! Indexed access into half-edge arrays and vertex incidence lists is the
//! canonical idiom for DCEL traversal — all `arr[i]` sites use indices
//! bounded by `half_edges.len()` (twin-pair construction). Matches the
//! numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::boolean::{GraphEdge, IntersectionGraph};
use super::{OperationError, OperationResult};
use crate::math::{Tolerance, Vector3};
use crate::primitives::{
    edge::EdgeId,
    surface::{Surface, SurfaceId, SurfaceType},
    topology_builder::BRepModel,
    vertex::VertexId,
};
use std::collections::HashMap;

/// Stable newtype for half-edge indices into [`Arrangement::half_edges`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct HalfEdgeId(pub(super) u32);

impl HalfEdgeId {
    const UNSET: Self = Self(u32::MAX);

    #[inline]
    fn index(self) -> usize {
        self.0 as usize
    }
}

/// Half-edge record in the planar-arrangement DCEL.
#[derive(Clone, Debug)]
pub(super) struct HalfEdge {
    /// Vertex this half-edge starts at.
    pub(super) origin: VertexId,
    /// Opposite half-edge of the same undirected edge.
    pub(super) twin: HalfEdgeId,
    /// Next half-edge on the same face (CCW under the surface normal).
    pub(super) next: HalfEdgeId,
    /// Underlying `BRepModel::edges` id (shared with twin).
    pub(super) edge_id: EdgeId,
    /// True if this half-edge traverses its underlying edge in the native
    /// direction (`start_vertex` → `end_vertex`).
    pub(super) forward: bool,
}

/// Planar-arrangement DCEL built from an [`IntersectionGraph`].
pub(super) struct Arrangement {
    pub(super) half_edges: Vec<HalfEdge>,
}

impl Arrangement {
    /// Number of half-edges (always even in a closed DCEL).
    #[inline]
    pub(super) fn len(&self) -> usize {
        self.half_edges.len()
    }

    #[inline]
    fn get(&self, id: HalfEdgeId) -> &HalfEdge {
        &self.half_edges[id.index()]
    }
}

/// Build the planar-arrangement DCEL for a face being split by cutting
/// curves.
///
/// Only graph edges with both endpoints resolved (non-zero `start_vertex`
/// and `end_vertex`) are included. Each valid undirected edge emits exactly
/// two half-edges (forward + reverse, twins of each other).
///
/// After emission, outgoing half-edges at each vertex are sorted by their
/// tangent direction projected into the surface's tangent plane, producing
/// a deterministic CCW cyclic order under the surface normal. Finally
/// `next` pointers are wired using the standard DCEL formula.
pub(super) fn build_arrangement(
    graph: &IntersectionGraph,
    model: &BRepModel,
    surface_id: SurfaceId,
) -> OperationResult<Arrangement> {
    // ------------------------------------------------------------------
    // 1. Fetch surface (used for per-vertex tangent-frame computation).
    // ------------------------------------------------------------------
    let surface = model
        .surfaces
        .get(surface_id)
        .ok_or_else(|| OperationError::InvalidInput {
            parameter: "surface_id".to_string(),
            expected: "valid surface ID".to_string(),
            received: format!("{surface_id:?}"),
        })?;

    // ------------------------------------------------------------------
    // 2. Emit two half-edges per valid undirected graph edge.
    // ------------------------------------------------------------------
    let mut half_edges: Vec<HalfEdge> = Vec::new();
    let mut outgoing: HashMap<VertexId, Vec<HalfEdgeId>> = HashMap::new();

    // Iterate in edge-id order for deterministic construction; this
    // matters for angular-sort tie-breaking under HashMap shuffle.
    let mut edge_ids: Vec<EdgeId> = graph
        .edges
        .iter()
        .filter(|(_, ge)| ge.start_vertex != u32::MAX && ge.end_vertex != u32::MAX)
        .map(|(&eid, _)| eid)
        .collect();
    edge_ids.sort_unstable();

    for edge_id in edge_ids {
        let ge: &GraphEdge = &graph.edges[&edge_id];
        // Skip zero-length/degenerate edges (start == end).
        if ge.start_vertex == ge.end_vertex {
            continue;
        }

        let fwd_id = HalfEdgeId(half_edges.len() as u32);
        let rev_id = HalfEdgeId(half_edges.len() as u32 + 1);

        half_edges.push(HalfEdge {
            origin: ge.start_vertex,
            twin: rev_id,
            next: HalfEdgeId::UNSET,
            edge_id,
            forward: true,
        });
        half_edges.push(HalfEdge {
            origin: ge.end_vertex,
            twin: fwd_id,
            next: HalfEdgeId::UNSET,
            edge_id,
            forward: false,
        });

        outgoing.entry(ge.start_vertex).or_default().push(fwd_id);
        outgoing.entry(ge.end_vertex).or_default().push(rev_id);
    }

    // ------------------------------------------------------------------
    // 3. Per-vertex angular sort in the surface's tangent plane.
    // ------------------------------------------------------------------
    // A tangent angle is computed for every outgoing half-edge; the list
    // is then sorted by ascending angle. Ties (numerically coincident
    // directions) fall back to the underlying edge id for determinism.
    let frame_tol = Tolerance::default();
    for (&vid, hes) in outgoing.iter_mut() {
        if hes.len() < 2 {
            continue; // Nothing to sort.
        }

        let pos = match model.vertices.get_position(vid) {
            Some(p) => Vector3::new(p[0], p[1], p[2]),
            None => continue,
        };

        // Compute orthonormal (e1, e2) tangent frame at this vertex.
        let (e1, e2) = match tangent_frame_at(surface, &pos, frame_tol) {
            Some(frame) => frame,
            None => {
                // Degenerate surface evaluation — leave order unsorted.
                // Determinism still holds because the earlier push order
                // was itself sorted by edge id.
                continue;
            }
        };

        // Build (angle, edge_id, half_edge_id) tuples and sort.
        let mut keyed: Vec<(f64, EdgeId, HalfEdgeId)> = hes
            .iter()
            .copied()
            .filter_map(|h| {
                let he = &half_edges[h.index()];
                let tangent = half_edge_tangent(he, model)?;
                let a = tangent.dot(&e1);
                let b = tangent.dot(&e2);
                // `atan2` is defined for (0, 0) but we filtered zero
                // tangents above via `curve.tangent_at` failure. If both
                // components are infinitesimal we still get a finite
                // angle (atan2(0,0) == 0 on IEEE 754), which is harmless.
                Some((b.atan2(a), he.edge_id, h))
            })
            .collect();
        keyed.sort_by(|a, b| {
            a.0.partial_cmp(&b.0)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.1.cmp(&b.1))
        });

        if keyed.len() == hes.len() {
            *hes = keyed.into_iter().map(|(_, _, h)| h).collect();
        } else {
            // Some half-edges failed to produce a tangent (degenerate
            // curve at vertex). Keep the sortable prefix and append the
            // unsortable suffix deterministically by edge id.
            let sorted: Vec<HalfEdgeId> = keyed.into_iter().map(|(_, _, h)| h).collect();
            let mut rest: Vec<HalfEdgeId> = hes
                .iter()
                .copied()
                .filter(|h| !sorted.contains(h))
                .collect();
            rest.sort_by_key(|h| half_edges[h.index()].edge_id);
            hes.clear();
            hes.extend(sorted);
            hes.extend(rest);
        }
    }

    // ------------------------------------------------------------------
    // 4. Wire `next` pointers: next(h) = outgoing[v][(i - 1 + k) mod k],
    //    where v = twin(h).origin, i = index_of(twin(h)), k = degree(v).
    //    This selects the outgoing half-edge immediately CW-previous to
    //    twin(h), i.e. the next edge on the face to the LEFT of h.
    // ------------------------------------------------------------------
    let he_count = half_edges.len();
    for h_idx in 0..he_count {
        let twin_id = half_edges[h_idx].twin;
        let v_end = half_edges[twin_id.index()].origin;

        let ring = match outgoing.get(&v_end) {
            Some(r) => r,
            None => continue,
        };
        let k = ring.len();
        if k == 0 {
            continue;
        }
        let i = match ring.iter().position(|&x| x == twin_id) {
            Some(i) => i,
            None => continue,
        };
        let next_idx = (i + k - 1) % k;
        half_edges[h_idx].next = ring[next_idx];
    }

    Ok(Arrangement { half_edges })
}

/// Walk minimal-face cycles in the arrangement and return each as a list
/// of `(underlying edge id, forward)` pairs.
///
/// `forward` is the half-edge's `forward` bit: `true` if the cycle walks
/// the underlying edge in its native start→end direction, `false` if it
/// walks end→start. Downstream consumers (loop construction in
/// `build_shells_from_faces`, classification, offset, sweep) use this bit
/// to assemble loops with correct edge orientation; without it the kernel
/// silently corrupts loop topology by hard-coding `forward=true`.
///
/// Faces with non-positive signed area (outer face, CW under the surface
/// normal) are discarded. Dangling-edge detours — half-edges whose twin is
/// walked immediately next in the same cycle — are stripped from the
/// boundary edge list before emission.
pub(super) fn extract_regions(
    arr: &Arrangement,
    model: &BRepModel,
    surface: &dyn Surface,
) -> Vec<Vec<(EdgeId, bool)>> {
    let tol = Tolerance::default();
    let mut visited = vec![false; arr.len()];
    let mut regions: Vec<Vec<(EdgeId, bool)>> = Vec::new();

    for start in 0..arr.len() {
        if visited[start] {
            continue;
        }
        let start_id = HalfEdgeId(start as u32);
        let mut he_cycle: Vec<HalfEdgeId> = Vec::new();
        let mut cur = start_id;

        // Hard walk limit to defend against malformed arrangements (a
        // correctly-wired DCEL always terminates after visiting each
        // half-edge at most twice; 4x is a very loose guard).
        let walk_limit = arr.len() * 4 + 8;
        let mut steps = 0;
        loop {
            if visited[cur.index()] {
                break;
            }
            visited[cur.index()] = true;
            he_cycle.push(cur);

            let nxt = arr.get(cur).next;
            if nxt == HalfEdgeId::UNSET {
                // Broken wiring — abandon this cycle.
                he_cycle.clear();
                break;
            }
            if nxt == start_id {
                break;
            }
            cur = nxt;
            steps += 1;
            if steps > walk_limit {
                he_cycle.clear();
                break;
            }
        }

        if he_cycle.len() < 2 {
            continue;
        }

        // Strip dangling detours: a dangling edge's two halves appear
        // consecutively in the cycle (next(h_MR) = h_RM when R has
        // degree 1). Repeatedly collapse such pairs.
        let trimmed = strip_dangling_pairs(&he_cycle, arr);

        if trimmed.len() < 3 {
            tracing::debug!(
                "extract_regions: cycle of {} stripped to {} (<3) — discarded",
                he_cycle.len(),
                trimmed.len()
            );
            continue;
        }

        // Compute signed area in the tangent plane of the cycle's
        // centroid. Outer face (CW under normal) has negative signed
        // area and is discarded. Zero-area cycles (pure lollipops) are
        // also discarded.
        let signed = signed_area_of_cycle(&trimmed, arr, model, surface, tol);
        tracing::debug!(
            "extract_regions: cycle of {} (trimmed {}) signed_area={:.4}",
            he_cycle.len(),
            trimmed.len(),
            signed
        );
        if signed <= tol.distance() * tol.distance() {
            continue;
        }

        // Collect underlying edge ids + half-edge forward bits in walk
        // order, deduping consecutive duplicates that may remain after
        // trimming. Two consecutive entries with the same edge_id can
        // arise from arrangement-vertex stitching where the same edge
        // appears twice; in that case the second occurrence's forward
        // bit is the meaningful one for the cycle and we keep the first
        // occurrence (matching the previous, edge-id-only behavior).
        let mut edges: Vec<(EdgeId, bool)> = Vec::with_capacity(trimmed.len());
        for &h in &trimmed {
            let he = arr.get(h);
            let eid = he.edge_id;
            if edges.last().map(|(e, _)| *e) != Some(eid) {
                edges.push((eid, he.forward));
            }
        }
        if edges.first().map(|(e, _)| *e) == edges.last().map(|(e, _)| *e) && edges.len() > 1 {
            edges.pop();
        }
        if edges.len() < 3 {
            continue;
        }

        regions.push(edges);
    }

    regions
}

// ---------------------------------------------------------------------
// Helpers.
// ---------------------------------------------------------------------

/// Build an orthonormal (e1, e2) tangent-plane basis at a 3D point on a
/// surface via Gram-Schmidt on (du, dv). Returns `None` on degenerate
/// surface evaluation.
fn tangent_frame_at(
    surface: &dyn Surface,
    pos: &Vector3,
    tol: Tolerance,
) -> Option<(Vector3, Vector3)> {
    let (u, v) = surface.closest_point(pos, tol).ok()?;
    let sp = surface.evaluate_full(u, v).ok()?;
    let e1 = sp.du.normalize().ok()?;
    let dv_perp = sp.dv - e1 * sp.dv.dot(&e1);
    let e2 = dv_perp.normalize().ok()?;
    // Ensure (e1, e2) is right-handed under the surface normal so that
    // angular sort is CCW as seen from the positive-normal side. For
    // faces with Reversed orientation the kernel currently stores the
    // same surface without flipping the normal — the existing
    // classification pipeline is normal-agnostic and re-derives
    // inside/outside via ray casting, so we keep the native orientation
    // here (same convention the previous `extract_face_loops` used).
    let cross = e1.cross(&e2);
    if cross.dot(&sp.normal) < 0.0 {
        Some((e1, -e2))
    } else {
        Some((e1, e2))
    }
}

/// 3D tangent of the underlying edge at this half-edge's origin vertex,
/// pointing away from the origin. Returns `None` if the curve/edge is
/// missing or the tangent is degenerate.
fn half_edge_tangent(he: &HalfEdge, model: &BRepModel) -> Option<Vector3> {
    let edge = model.edges.get(he.edge_id)?;
    let curve = model.curves.get(edge.curve_id)?;
    let t_at_origin = if he.forward {
        edge.param_range.start
    } else {
        edge.param_range.end
    };
    let tan = curve.tangent_at(t_at_origin).ok()?;
    // `tangent_at` returns the normalized curve tangent in the native
    // direction; flip for reverse half-edges so the vector points away
    // from the half-edge's origin vertex.
    if he.forward {
        Some(tan)
    } else {
        Some(-tan)
    }
}

/// Remove consecutive dangling-edge pairs (h, twin(h)) from a half-edge
/// cycle, repeating until the cycle is stable.
///
/// A dangling edge `MR` with degree-1 endpoint `R` wires as
/// `next(h_MR) = h_RM` (the twin), so both halves appear consecutively in
/// the cycle walk. Stripping them collapses the lollipop excursion while
/// preserving the surrounding region's boundary. Wraparound pairs
/// (last-element ↔ first-element) are also handled by rotating the cycle
/// after each linear sweep.
fn strip_dangling_pairs(cycle: &[HalfEdgeId], arr: &Arrangement) -> Vec<HalfEdgeId> {
    let mut v: Vec<HalfEdgeId> = cycle.to_vec();
    loop {
        // Linear sweep: drop adjacent twin pairs.
        let mut changed = false;
        let mut out: Vec<HalfEdgeId> = Vec::with_capacity(v.len());
        let mut i = 0;
        while i < v.len() {
            if i + 1 < v.len() && arr.get(v[i]).twin == v[i + 1] {
                i += 2;
                changed = true;
            } else {
                out.push(v[i]);
                i += 1;
            }
        }
        v = out;
        if v.is_empty() {
            break;
        }
        // Wraparound: last ↔ first.
        if v.len() >= 2 && arr.get(v[v.len() - 1]).twin == v[0] {
            v.remove(v.len() - 1);
            if !v.is_empty() {
                v.remove(0);
            }
            changed = true;
        }
        if !changed {
            break;
        }
    }
    v
}

/// Signed area of a half-edge cycle on the underlying surface.
///
/// For planar surfaces the cycle is projected into the tangent plane at
/// the cycle's 3D centroid (positive ⇒ CCW under the surface normal).
///
/// For non-planar surfaces (cylinder, cone, sphere, NURBS, …) a flat
/// tangent-plane projection is unsound: a closed cycle that wraps a
/// cylindrical band, for example, has all its vertices on the seam line
/// (because the angular midpoint of every closed cap-circle is at
/// `u = π`, which lies on `y = 0` together with `u = 0` itself), so the
/// projection collapses to zero and every region is incorrectly
/// rejected as "outer face". Instead, we work in the surface's
/// parametric `(u, v)` space and apply a 2D shoelace there. This is
/// exact on parametric surfaces because the cycle's interior in 3D
/// corresponds (modulo the seam) to a simple polygon in `(u, v)`.
///
/// For periodic surfaces the seam introduces an angular ambiguity:
/// `closest_point` may return either `u = 0` or `u = period` for a
/// vertex on the seam. We resolve this by **edge-midpoint anchoring**:
/// each edge's midpoint is sampled, its `(u, v)` is computed, and the
/// successive vertex's `u` is unwrapped to whichever periodic copy
/// makes the midpoint lie on the linear path between the two endpoint
/// `u` values. This walks the cycle continuously around the seam
/// without ambiguity.
fn signed_area_of_cycle(
    cycle: &[HalfEdgeId],
    arr: &Arrangement,
    model: &BRepModel,
    surface: &dyn Surface,
    tol: Tolerance,
) -> f64 {
    if cycle.len() < 3 {
        return 0.0;
    }

    // Collect vertex positions in walk order.
    let positions: Vec<Vector3> = cycle
        .iter()
        .filter_map(|&h| model.vertices.get_position(arr.get(h).origin))
        .map(|p| Vector3::new(p[0], p[1], p[2]))
        .collect();
    if positions.len() < 3 {
        return 0.0;
    }

    // Planar surfaces: use the legacy tangent-plane projection at the
    // 3D centroid. This is the optimal frame for a planar polygon and
    // matches all of the existing planar-boolean tests.
    if surface.surface_type() == SurfaceType::Plane {
        let n = positions.len() as f64;
        let centroid = positions.iter().fold(Vector3::ZERO, |acc, p| acc + *p) / n;
        let (e1, e2) = match tangent_frame_at(surface, &centroid, tol) {
            Some(f) => f,
            None => return 0.0,
        };

        let mut area2 = 0.0;
        for i in 0..positions.len() {
            let a = positions[i] - centroid;
            let b = positions[(i + 1) % positions.len()] - centroid;
            let ax = a.dot(&e1);
            let ay = a.dot(&e2);
            let bx = b.dot(&e1);
            let by = b.dot(&e2);
            area2 += ax * by - bx * ay;
        }
        return 0.5 * area2;
    }

    // Non-planar surfaces: work in parametric (u, v) with seam unwrap.
    let uvs = match unwrap_cycle_uv(cycle, arr, model, surface, tol) {
        Some(uvs) => uvs,
        None => return 0.0,
    };
    if uvs.len() < 3 {
        return 0.0;
    }

    let mut area2 = 0.0;
    for i in 0..uvs.len() {
        let (ax, ay) = uvs[i];
        let (bx, by) = uvs[(i + 1) % uvs.len()];
        area2 += ax * by - bx * ay;
    }
    0.5 * area2
}

/// Compute parametric `(u, v)` for every vertex in a cycle, unwrapped
/// across periodic seams using each edge's midpoint as a continuity
/// anchor.
///
/// For each successive vertex the candidate `u` (and `v`) values are
/// `u_raw + k · period` for `k ∈ {-1, 0, 1}` (only the `0` candidate is
/// considered for non-periodic axes). The candidate that minimises the
/// distance from the corresponding edge midpoint's parametric coordinate
/// is selected — this places the midpoint on the linear path between
/// the two endpoint `(u, v)` and resolves seam ambiguity without
/// requiring per-edge geometric guards.
fn unwrap_cycle_uv(
    cycle: &[HalfEdgeId],
    arr: &Arrangement,
    model: &BRepModel,
    surface: &dyn Surface,
    tol: Tolerance,
) -> Option<Vec<(f64, f64)>> {
    let n = cycle.len();
    if n < 2 {
        return None;
    }

    // Raw parametric coordinates at every cycle vertex.
    let mut raw: Vec<(f64, f64)> = Vec::with_capacity(n);
    for &h in cycle {
        let pos_arr = model.vertices.get_position(arr.get(h).origin)?;
        let pos = Vector3::new(pos_arr[0], pos_arr[1], pos_arr[2]);
        let uv = surface.closest_point(&pos, tol).ok()?;
        raw.push(uv);
    }

    // Edge-midpoint parametric coordinates: midpoint of the edge that
    // *starts* at the i-th cycle vertex (the edge represented by the
    // i-th half-edge in walk order).
    let mut mids: Vec<(f64, f64)> = Vec::with_capacity(n);
    for &h in cycle {
        let he = arr.get(h);
        let edge = model.edges.get(he.edge_id)?;
        let curve = model.curves.get(edge.curve_id)?;
        // Edge t = 0.5 → curve t via edge orientation/parameter range.
        let curve_t = edge.edge_to_curve_parameter(0.5);
        let mid_pt = curve.point_at(curve_t).ok()?;
        let uv = surface.closest_point(&mid_pt, tol).ok()?;
        mids.push(uv);
    }

    let period_u = surface.period_u();
    let period_v = surface.period_v();

    // Anchor first vertex — its raw (u, v) is taken as-is. To match
    // it against the first edge's midpoint we still allow the midpoint
    // to be unwrapped relative to the anchor below.
    let mut uvs: Vec<(f64, f64)> = Vec::with_capacity(n);
    uvs.push(raw[0]);

    for i in 1..n {
        let prev = uvs[i - 1];
        let mid_raw = mids[i - 1]; // midpoint of edge from vertex i-1 to vertex i
        let cand_raw = raw[i];

        // First, unwrap the midpoint relative to `prev`.
        let mid_u = nearest_periodic(prev.0, mid_raw.0, period_u);
        let mid_v = nearest_periodic(prev.1, mid_raw.1, period_v);

        // Then, unwrap the next vertex relative to `mid` so that the
        // midpoint sits on the linear path between prev and next.
        let next_u = nearest_periodic(mid_u, cand_raw.0, period_u);
        let next_v = nearest_periodic(mid_v, cand_raw.1, period_v);

        uvs.push((next_u, next_v));
    }

    Some(uvs)
}

/// Return `value + k · period` for the `k ∈ {-1, 0, 1}` that minimises
/// `|value + k · period - anchor|`. If `period` is `None` (surface is
/// not periodic on this axis), `value` is returned unchanged.
#[inline]
fn nearest_periodic(anchor: f64, value: f64, period: Option<f64>) -> f64 {
    match period {
        None => value,
        Some(p) if !(p > 0.0) => value,
        Some(p) => {
            let candidates = [value - p, value, value + p];
            let mut best = candidates[0];
            let mut best_d = (best - anchor).abs();
            for &c in &candidates[1..] {
                let d = (c - anchor).abs();
                if d < best_d {
                    best = c;
                    best_d = d;
                }
            }
            best
        }
    }
}

#[cfg(test)]
mod tests {
    //! DCEL primitives and helpers are exercised end-to-end by the
    //! tier-3 property tests in `boolean.rs`
    //! (`prop_tier3_intersection_bbox_within_both_inputs`,
    //! `prop_tier3_difference_bbox_within_minuend`), which drive
    //! `build_arrangement` + `extract_regions` through the full boolean
    //! pipeline with random box/box inputs. The focused test below
    //! guards the dangling-edge stripping helper — a pure-combinatorial
    //! routine that does not require a surface or model.

    use super::*;

    /// Build an `Arrangement` stub with manual twin wiring for pure
    /// combinatorial tests of `strip_dangling_pairs`.
    fn make_stub(pairs: &[(u32, u32)]) -> Arrangement {
        let mut half_edges = Vec::new();
        for &(a, b) in pairs {
            half_edges.push(HalfEdge {
                origin: 0,
                twin: HalfEdgeId(b),
                next: HalfEdgeId::UNSET,
                edge_id: a,
                forward: true,
            });
        }
        Arrangement { half_edges }
    }

    #[test]
    fn strip_keeps_clean_cycle() {
        // Four half-edges none of which are each other's twins.
        let arr = make_stub(&[(0, 99), (1, 99), (2, 99), (3, 99)]);
        let cycle = vec![HalfEdgeId(0), HalfEdgeId(1), HalfEdgeId(2), HalfEdgeId(3)];
        let out = strip_dangling_pairs(&cycle, &arr);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn strip_removes_consecutive_twin_pair() {
        // he 1 and he 2 are each other's twins (simulate dangling).
        let arr = make_stub(&[(0, 99), (1, 2), (2, 1), (3, 99)]);
        let cycle = vec![HalfEdgeId(0), HalfEdgeId(1), HalfEdgeId(2), HalfEdgeId(3)];
        let out = strip_dangling_pairs(&cycle, &arr);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], HalfEdgeId(0));
        assert_eq!(out[1], HalfEdgeId(3));
    }

    #[test]
    fn strip_handles_back_to_back_lollipops() {
        // Two nested dangling pairs in a row.
        let arr = make_stub(&[(0, 99), (1, 2), (2, 1), (3, 4), (4, 3), (5, 99)]);
        let cycle = vec![
            HalfEdgeId(0),
            HalfEdgeId(1),
            HalfEdgeId(2),
            HalfEdgeId(3),
            HalfEdgeId(4),
            HalfEdgeId(5),
        ];
        let out = strip_dangling_pairs(&cycle, &arr);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0], HalfEdgeId(0));
        assert_eq!(out[1], HalfEdgeId(5));
    }

    #[test]
    fn half_edge_id_unset_is_distinct() {
        assert_ne!(HalfEdgeId::UNSET, HalfEdgeId(0));
        assert_ne!(HalfEdgeId::UNSET, HalfEdgeId(12345));
    }
}
