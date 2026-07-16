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
use crate::math::vector2::Vector2;
use crate::math::{
    circular_order, orient2d, polygon_orientation_2d, Orientation, Tolerance, Vector3,
};
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

        // Build (tangent-dir, offset-dir, edge_id, half_edge_id) tuples and
        // sort by the EXACT circular order of the tangent directions in the
        // (e1, e2) frame (EXACT PREDICATES Slice 3, census row #6:
        // `math::circular_order` — quadrant split + exact orient2d cross
        // sign; Shewchuk 1997). The former `atan2` sort key could COLLIDE for
        // sub-ulp-distinct directions (two near-parallel tangents mapping to
        // the same f64 angle), silently handing the cyclic order to the
        // id-based tie-break — and its `partial_cmp … NaN→Equal` arm was not
        // a strict weak order. The exact key ties ONLY for bit-equal
        // directions; the order is a pure function of the tangent geometry.
        // (The order starts at the frame's +e1 axis instead of atan2's −e1
        // branch cut; DCEL `next` wiring below consumes only the CYCLIC
        // order, which is rotation-invariant.)
        let mut keyed: Vec<(Vector2, Vector2, EdgeId, HalfEdgeId)> = hes
            .iter()
            .copied()
            .filter_map(|h| {
                let he = &half_edges[h.index()];
                let tangent = half_edge_tangent(he, model)?;
                let dir = Vector2::new(tangent.dot(&e1), tangent.dot(&e2));
                // Second-order (curvature) tie-break for EXACT tangencies: two
                // half-edges can leave this vertex with a BIT-IDENTICAL first-
                // order tangent — e.g. a circle arc inscribed-tangent to a
                // straight boundary edge at an axis-aligned point, where both
                // tangents are exactly (0,0,±1) (#82). The primary key is then
                // Equal and the sort would fall through to `edge_id`, which is
                // assigned in non-deterministic edge-creation order, shuffling
                // the ring and the extracted loops run-to-run. Disambiguate by
                // the direction to a point a small parameter-step INTO the edge:
                // a straight edge keeps the tangent direction, a curved one
                // rotates by its curvature — separating them GEOMETRICALLY and
                // id-invariantly.
                let dir2 = half_edge_offset_dir(he, model, pos, &e1, &e2).unwrap_or(dir);
                Some((dir, dir2, he.edge_id, h))
            })
            .collect();
        keyed.sort_by(|a, b| {
            circular_order(&a.0, &b.0)
                .then_with(|| offset_tie_order(&a.1, &b.1))
                .then_with(|| a.2.cmp(&b.2))
        });

        if keyed.len() == hes.len() {
            *hes = keyed.into_iter().map(|(_, _, _, h)| h).collect();
        } else {
            // Some half-edges failed to produce a tangent (degenerate
            // curve at vertex). Keep the sortable prefix and append the
            // unsortable suffix deterministically by edge id.
            let sorted: Vec<HalfEdgeId> = keyed.into_iter().map(|(_, _, _, h)| h).collect();
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

        // A ≥3-edge cycle is the normal polygonal face. A 2-edge cycle is a
        // valid "lune" — a region bounded by a curved edge and a chord that
        // share both endpoints (the inside-arc piece when an arc cut is
        // imprinted on a planar face, e.g. a cylinder section on a box cap that
        // the intersection must keep) — ONLY when its two edges are DISTINCT;
        // a 2-edge cycle of one edge traversed both ways is a degenerate
        // lollipop. Anything below 2 is dropped.
        let is_lune =
            trimmed.len() == 2 && arr.get(trimmed[0]).edge_id != arr.get(trimmed[1]).edge_id;
        if trimmed.len() < 3 && !is_lune {
            tracing::debug!(
                "extract_regions: cycle of {} stripped to {} (<3) — discarded",
                he_cycle.len(),
                trimmed.len()
            );
            continue;
        }

        // Project the cycle into 2D (tangent plane at the cycle's centroid
        // for planar surfaces, unwrapped (u, v) otherwise; the lune samples
        // its curved edge to recover the bulge its two shared vertices alone
        // would report as zero) and DECOMPOSE the former fused
        // `signed <= tol²` compare into its two owners (EXACT PREDICATES
        // Slice 3, census row #7; spec §3.1 boundary-contract rule 3):
        //
        //   1. Regime E — "is this the outer face?" is the EXACT shoelace
        //      SIGN of the projected cycle (`math::polygon_orientation_2d`):
        //      CW-under-normal and exactly-degenerate cycles are discarded
        //      by sign, zero epsilons.
        //   2. Regime T — "is this a sliver below modeling significance?" is
        //      the named REGION_SLIVER_AREA gate on the f64 area magnitude.
        let poly_2d = if is_lune {
            densified_cycle_polygon(&trimmed, arr, model, surface, tol)
        } else {
            cycle_polygon_2d(&trimmed, arr, model, surface, tol)
        };
        let Some(poly_2d) = poly_2d else {
            continue; // unprojectable cycle — matches the former 0.0-area drop
        };
        let signed = shoelace_area_f64(&poly_2d);
        tracing::debug!(
            "extract_regions: cycle of {} (trimmed {}) signed_area={:.4}",
            he_cycle.len(),
            trimmed.len(),
            signed
        );
        if polygon_orientation_2d(&poly_2d) != Orientation::CounterClockwise {
            continue; // outer face (CW) or exactly-zero cycle — exact sign
        }
        if signed <= region_sliver_area_gate(tol) {
            continue; // Regime-T sliver gate (named, separate from the sign)
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
        // ≥3 edges normally; a 2-edge lune survives (its two distinct edges and
        // positive sampled area were already validated above).
        let min_edges = if is_lune { 2 } else { 3 };
        if edges.len() < min_edges {
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

/// Direction (in the vertex's `(e1, e2)` tangent frame) from the vertex to a
/// point a small parameter-step INTO this half-edge. Used as the second-order
/// tie-break in the angular sort: when two half-edges share a bit-identical
/// first-order tangent (an exact tangency), this curvature-aware direction
/// separates them geometrically — a straight edge keeps the tangent
/// direction, a curved one rotates by its curvature. Deterministic and
/// independent of edge-id assignment order. `origin` is the vertex position.
fn half_edge_offset_dir(
    he: &HalfEdge,
    model: &BRepModel,
    origin: Vector3,
    e1: &Vector3,
    e2: &Vector3,
) -> Option<Vector2> {
    let edge = model.edges.get(he.edge_id)?;
    let curve = model.curves.get(edge.curve_id)?;
    let (t0, t1) = (edge.param_range.start, edge.param_range.end);
    let delta = 1.0e-3_f64;
    let t = if he.forward {
        t0 + (t1 - t0) * delta
    } else {
        t1 - (t1 - t0) * delta
    };
    let p = curve.point_at(t).ok()?;
    let dir = Vector3::new(p.x - origin.x, p.y - origin.y, p.z - origin.z);
    let a = dir.dot(e1);
    let b = dir.dot(e2);
    if a.abs() < 1.0e-15 && b.abs() < 1.0e-15 {
        return None; // offset point coincides with the vertex — no information
    }
    Some(Vector2::new(a, b))
}

/// EXACT second-order tie order for two half-edges whose first-order tangents
/// are bit-identical: the local rotation of one offset direction against the
/// other, via the exact [`orient2d`] cross sign. Both offset directions
/// deviate from the SHARED tangent direction by less than a quarter turn (a
/// short parameter-step chord cannot swing past the tangent's normal plane
/// for any sanely-parameterized edge curve), so within that window
/// `cross(a, b) > 0` ⇔ `a` sits at the smaller CCW angle ⇔ `a` first.
/// `Collinear` (bit-equal offset directions too) falls through to the
/// caller's deterministic `edge_id` fallback. Unlike the former linear
/// `atan2(offset)` compare this has no branch cut: a tangency pointing along
/// the frame's −e1 axis orders the same as any other direction.
fn offset_tie_order(a: &Vector2, b: &Vector2) -> std::cmp::Ordering {
    match orient2d(&Vector2::ZERO, a, b) {
        Orientation::CounterClockwise => std::cmp::Ordering::Less,
        Orientation::Clockwise => std::cmp::Ordering::Greater,
        Orientation::Collinear => std::cmp::Ordering::Equal,
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

/// Regime-T sliver gate of `extract_regions` (EXACT PREDICATES Slice 3; spec
/// §3.1 boundary-contract rule 3): the area magnitude below which a
/// positively-oriented region is dropped as modeling-insignificant. Named and
/// owned separately from the exact orientation SIGN decision — the former
/// single `signed <= tol²` compare fused both questions. Derived from the
/// modeling tolerance: the area of a τ×τ square (Slice 5's tolerance
/// authority owns the derivation rule).
#[inline]
fn region_sliver_area_gate(tol: Tolerance) -> f64 {
    tol.distance() * tol.distance()
}

/// Plain f64 shoelace signed area of a projected 2D polygon — the MAGNITUDE
/// companion of the exact sign (`math::polygon_orientation_2d`); consumed
/// only by the Regime-T sliver gate and the trace output.
fn shoelace_area_f64(poly: &[(f64, f64)]) -> f64 {
    let n = poly.len();
    if n < 3 {
        return 0.0;
    }
    let mut area2 = 0.0;
    for i in 0..n {
        let (ax, ay) = poly[i];
        let (bx, by) = poly[(i + 1) % n];
        area2 += ax * by - bx * ay;
    }
    0.5 * area2
}

/// 2D projection of a cycle whose edges may be CURVED, computed by sampling
/// each half-edge's underlying curve (in walk order) into a dense polyline
/// projected into the tangent plane at the centroid. Unlike
/// [`cycle_polygon_2d`] — which uses only the cycle's corner vertices, so a
/// 2-edge lune (an arc and its chord share both endpoints) collapses to zero
/// area — this recovers the bulge. Used for the 2-edge lune; the ≥3-edge path
/// keeps the cheaper vertex polygon. `None` ⇒ the cycle cannot be projected
/// (missing geometry / degenerate frame), which the caller treats as a
/// discarded zero-area cycle.
fn densified_cycle_polygon(
    cycle: &[HalfEdgeId],
    arr: &Arrangement,
    model: &BRepModel,
    surface: &dyn Surface,
    tol: Tolerance,
) -> Option<Vec<(f64, f64)>> {
    const SAMPLES_PER_EDGE: usize = 8;
    let mut pts: Vec<Vector3> = Vec::new();
    for &h in cycle {
        let he = arr.get(h);
        let edge = model.edges.get(he.edge_id)?;
        let curve = model.curves.get(edge.curve_id)?;
        let (t0, t1) = (edge.param_range.start, edge.param_range.end);
        // Walk the edge in the half-edge's direction; drop the final point
        // (shared with the next edge's first sample) to avoid duplicates.
        for k in 0..SAMPLES_PER_EDGE {
            let f = k as f64 / SAMPLES_PER_EDGE as f64;
            let t = if he.forward {
                t0 + (t1 - t0) * f
            } else {
                t1 + (t0 - t1) * f
            };
            if let Ok(p) = curve.evaluate(t) {
                pts.push(Vector3::new(p.position.x, p.position.y, p.position.z));
            }
        }
    }
    if pts.len() < 3 {
        return None;
    }
    let n = pts.len() as f64;
    let centroid = pts.iter().fold(Vector3::ZERO, |acc, p| acc + *p) / n;
    let (e1, e2) = tangent_frame_at(surface, &centroid, tol)?;
    Some(
        pts.iter()
            .map(|p| {
                let d = *p - centroid;
                (d.dot(&e1), d.dot(&e2))
            })
            .collect(),
    )
}

/// 2D projection of a half-edge cycle's corner vertices on the underlying
/// surface — the polygon whose shoelace sign/area drive region keep/drop.
///
/// For planar surfaces the cycle is projected into the tangent plane at
/// the cycle's 3D centroid (positive shoelace ⇒ CCW under the surface
/// normal).
///
/// For non-planar surfaces (cylinder, cone, sphere, NURBS, …) a flat
/// tangent-plane projection is unsound: a closed cycle that wraps a
/// cylindrical band, for example, has all its vertices on the seam line
/// (because the angular midpoint of every closed cap-circle is at
/// `u = π`, which lies on `y = 0` together with `u = 0` itself), so the
/// projection collapses to zero and every region is incorrectly
/// rejected as "outer face". Instead, we work in the surface's
/// parametric `(u, v)` space and shoelace there. This is sound on
/// parametric surfaces because the cycle's interior in 3D corresponds
/// (modulo the seam) to a simple polygon in `(u, v)`.
///
/// For periodic surfaces the seam introduces an angular ambiguity:
/// `closest_point` may return either `u = 0` or `u = period` for a
/// vertex on the seam. We resolve this by **edge-midpoint anchoring**:
/// each edge's midpoint is sampled, its `(u, v)` is computed, and the
/// successive vertex's `u` is unwrapped to whichever periodic copy
/// makes the midpoint lie on the linear path between the two endpoint
/// `u` values. This walks the cycle continuously around the seam
/// without ambiguity.
fn cycle_polygon_2d(
    cycle: &[HalfEdgeId],
    arr: &Arrangement,
    model: &BRepModel,
    surface: &dyn Surface,
    tol: Tolerance,
) -> Option<Vec<(f64, f64)>> {
    if cycle.len() < 3 {
        return None;
    }

    // Collect vertex positions in walk order.
    let positions: Vec<Vector3> = cycle
        .iter()
        .filter_map(|&h| model.vertices.get_position(arr.get(h).origin))
        .map(|p| Vector3::new(p[0], p[1], p[2]))
        .collect();
    if positions.len() < 3 {
        return None;
    }

    // Planar surfaces: use the legacy tangent-plane projection at the
    // 3D centroid. This is the optimal frame for a planar polygon and
    // matches all of the existing planar-boolean tests.
    if surface.surface_type() == SurfaceType::Plane {
        let n = positions.len() as f64;
        let centroid = positions.iter().fold(Vector3::ZERO, |acc, p| acc + *p) / n;
        let (e1, e2) = tangent_frame_at(surface, &centroid, tol)?;
        return Some(
            positions
                .iter()
                .map(|p| {
                    let d = *p - centroid;
                    (d.dot(&e1), d.dot(&e2))
                })
                .collect(),
        );
    }

    // Non-planar surfaces: work in parametric (u, v) with seam unwrap.
    let uvs = unwrap_cycle_uv(cycle, arr, model, surface, tol)?;
    if uvs.len() < 3 {
        return None;
    }
    Some(uvs)
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

    let period_u = effective_period_u(surface);
    let period_v = effective_period_v(surface);

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

/// Period of the surface's U domain **as the boolean face arrangement needs
/// it**: the U-span over which a cut cycle may wrap across the seam.
///
/// `surface.period_u()` reports a period only when the surface advertises
/// itself as periodic/closed in U. That signal is correct for the analytic
/// primitives and the CLAMPED-and-repeated NURBS skin (first control row ==
/// last). It is **silent** for a genuinely PERIODIC NURBS built by
/// `skin_surface_periodic_u`, whose control net is WRAPPED (`m_u = n_u +
/// degree_u` rows, the first `degree_u` duplicated at the end) so the
/// first/last control rows differ even though `S(u_min, v) == S(u_max, v)`
/// for every `v` — a smooth closed seam. Without a period the cycle-area
/// unwrap leaves a seam-crossing complement cycle wrapped, so its shoelace
/// area comes out tiny-and-negative and `extract_regions` discards it as the
/// outer face — the lofted-barrel boolean then drops the entire freeform wall
/// (#23.3 regression).
///
/// This helper takes the advertised period when present, and otherwise probes
/// the surface GEOMETRICALLY: if `S(u_min, v) == S(u_max, v)` at several `v`
/// samples the domain is closed in U and the span is its period. The probe is
/// confined to this module (the arrangement), so the surface's globally-
/// reported periodicity — and therefore its tessellation routing — is
/// unchanged.
fn effective_period_u(surface: &dyn Surface) -> Option<f64> {
    if let Some(p) = surface.period_u() {
        return Some(p);
    }
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    if seam_closes_in_u(surface, u_min, u_max, v_min, v_max) {
        Some(u_max - u_min)
    } else {
        None
    }
}

/// V-axis analogue of [`effective_period_u`].
fn effective_period_v(surface: &dyn Surface) -> Option<f64> {
    if let Some(p) = surface.period_v() {
        return Some(p);
    }
    let ((u_min, u_max), (v_min, v_max)) = surface.parameter_bounds();
    if seam_closes_in_v(surface, u_min, u_max, v_min, v_max) {
        Some(v_max - v_min)
    } else {
        None
    }
}

/// Geometric closure probe: `true` when `S(u_min, v) == S(u_max, v)` across a
/// short `v` sweep, i.e. the surface seams onto itself in U. A non-degenerate
/// U-span is required; any evaluation failure is treated as "not closed".
fn seam_closes_in_u(surface: &dyn Surface, u_min: f64, u_max: f64, v_min: f64, v_max: f64) -> bool {
    if !(u_max > u_min) {
        return false;
    }
    const SAMPLES: usize = 5;
    const SEAM_TOL: f64 = 1e-7;
    for k in 0..=SAMPLES {
        let t = k as f64 / SAMPLES as f64;
        let v = v_min + (v_max - v_min) * t;
        let (a, b) = match (surface.point_at(u_min, v), surface.point_at(u_max, v)) {
            (Ok(a), Ok(b)) => (a, b),
            _ => return false,
        };
        if (a - b).magnitude() > SEAM_TOL {
            return false;
        }
    }
    true
}

/// V-axis analogue of [`seam_closes_in_u`]: `S(u, v_min) == S(u, v_max)`.
fn seam_closes_in_v(surface: &dyn Surface, u_min: f64, u_max: f64, v_min: f64, v_max: f64) -> bool {
    if !(v_max > v_min) {
        return false;
    }
    const SAMPLES: usize = 5;
    const SEAM_TOL: f64 = 1e-7;
    for k in 0..=SAMPLES {
        let t = k as f64 / SAMPLES as f64;
        let u = u_min + (u_max - u_min) * t;
        let (a, b) = match (surface.point_at(u, v_min), surface.point_at(u, v_max)) {
            (Ok(a), Ok(b)) => (a, b),
            _ => return false,
        };
        if (a - b).magnitude() > SEAM_TOL {
            return false;
        }
    }
    true
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

    /// EXACT PREDICATES Slice 3 RED (census row #6): near-parallel edge
    /// directions at a shared vertex corrupt DCEL loop wiring under the
    /// former `atan2` sort key.
    ///
    /// Construction: two triangles T1 = (O, P1, P2) and T2 = (O, P3, P4)
    /// sharing only the vertex O, whose adjacent legs O→P2 and O→P3 are
    /// sub-ulp-separated in angle. The pair is SEARCHED (deterministic seed)
    /// so that, mirroring the former float sort keys exactly:
    ///   * `atan2` COLLIDES on the two normalized tangents (identical f64
    ///     primary keys), while the exact cross sign still orders them
    ///     (O→P2 strictly before O→P3 in CCW order), and
    ///   * the former second-order key (`atan2` of the offset-point
    ///     direction) does NOT rescue the order (it reports ≥, i.e. wrong or
    ///     tied — and the edge-id fallback is arranged wrong by creating
    ///     T2's spoke first).
    /// Under the float sort the ring at O comes out cyclically wrong, `next`
    /// wiring crosses the two triangles, and the extracted regions are not
    /// the {T1}, {T2} partition — the module's documented failure class
    /// (loops spanning regions). The exact `circular_order` key decides the
    /// pair by the cross sign and never reaches the tie-break.
    ///
    /// RED evidence (2026-07-16): with the sort key hand-reverted to
    /// `atan2` + `partial_cmp`, this test FAILS (mutation proof); with the
    /// exact key it passes.
    #[test]
    fn near_parallel_spokes_extract_correct_regions() {
        use crate::math::Point3;
        use crate::primitives::curve::{Curve, Line, ParameterRange};
        use crate::primitives::edge::{Edge, EdgeOrientation};
        use crate::primitives::surface::Plane;
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        use std::cmp::Ordering as O;

        let origin = Point3::new(0.0, 0.0, 0.0);

        // Mirrors of the FORMER float sort keys at this call site.
        // Primary: atan2 of the normalized outgoing tangent's frame
        // components (frame = (X, Y) for the z=0 plane below). The tangent
        // of the outgoing half-edge toward P is `normalize(P - O)` for a
        // forward spoke and `-normalize(O - P)` for the reverse half-edge of
        // an incoming spoke — identical bits either way (negation is exact).
        let float_primary = |p: Point3| -> f64 {
            let t = Line::new(origin, p)
                .tangent_at(0.0)
                .expect("nonzero spoke tangent");
            t.y.atan2(t.x)
        };
        // Former secondary: atan2 of the direction to the point one
        // parameter-step (1e-3 of the range) INTO the half-edge.
        // For the forward spoke O→P3 (edge (O, P3), t = 0 + 1e-3):
        let float_secondary_fwd = |p: Point3| -> f64 {
            let q = Line::new(origin, p)
                .point_at(1.0e-3)
                .expect("line point_at");
            (q.y - origin.y).atan2(q.x - origin.x)
        };
        // For the reverse half-edge of the incoming spoke (P2, O)
        // (t = 1 − 1e-3, direction measured from O):
        let float_secondary_rev = |p: Point3| -> f64 {
            let q = Line::new(p, origin)
                .point_at(1.0 - 1.0e-3)
                .expect("line point_at");
            (q.y - origin.y).atan2(q.x - origin.x)
        };
        let frame_dir = |p: Point3| -> Vector2 {
            let t = Line::new(origin, p)
                .tangent_at(0.0)
                .expect("nonzero spoke tangent");
            Vector2::new(t.x, t.y)
        };

        // Deterministic search for the adversarial pair.
        let mut rng = StdRng::seed_from_u64(0xD0CE_1A11_5117_CE03);
        let mut found: Option<(Point3, Point3)> = None;
        for _ in 0..500_000u32 {
            let theta: f64 = rng.gen_range(0.25..1.25);
            let r2: f64 = rng.gen_range(5.0..20.0) * rng.gen_range(0.5..1.0);
            let r3: f64 = rng.gen_range(5.0..20.0) * rng.gen_range(0.5..1.0);
            // Sub-ulp CCW rotation of the second spoke.
            let dtheta: f64 = rng.gen_range(1.0e-19_f64..1.2e-16);
            let p2 = Point3::new(r2 * theta.cos(), r2 * theta.sin(), 0.0);
            let a3 = theta + dtheta;
            let p3 = Point3::new(r3 * a3.cos(), r3 * a3.sin(), 0.0);

            let d2 = frame_dir(p2);
            let d3 = frame_dir(p3);
            // Truth: O→P2 strictly before O→P3 (exact cross sign).
            if circular_order(&d2, &d3) != O::Less {
                continue;
            }
            // Former primary must collide (identical f64 angles).
            if float_primary(p2) != float_primary(p3) {
                continue;
            }
            // Former secondary must not rescue the order: wrong or tied.
            // (Tied falls to edge_id, which the construction arranges wrong.)
            if float_secondary_rev(p2) < float_secondary_fwd(p3) {
                continue;
            }
            found = Some((p2, p3));
            break;
        }
        let (p2, p3) = found.expect(
            "adversarial search exhausted: no atan2-colliding near-parallel \
             spoke pair found — widen the dtheta window",
        );

        // Far corners of the two triangle sectors (well clear of the pair).
        let p1 = Point3::new(10.0, -3.0, 0.0);
        let p4 = Point3::new(3.0, 10.0, 0.0);

        let mut m = BRepModel::new();
        let sid = m.surfaces.add(Box::new(
            Plane::new(origin, Vector3::Z, Vector3::X).expect("plane"),
        ));
        let vo = m.vertices.add(origin.x, origin.y, origin.z);
        let v1 = m.vertices.add(p1.x, p1.y, p1.z);
        let v2 = m.vertices.add(p2.x, p2.y, p2.z);
        let v3 = m.vertices.add(p3.x, p3.y, p3.z);
        let v4 = m.vertices.add(p4.x, p4.y, p4.z);

        let mut add_edge = |m: &mut BRepModel, a: (VertexId, Point3), b: (VertexId, Point3)| {
            let cid = m.curves.add(Box::new(Line::new(a.1, b.1)));
            m.edges.add(Edge::new(
                0,
                a.0,
                b.0,
                cid,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            ))
        };
        // Creation order arranges the edge-id tie-break WRONG for the former
        // float sort: T2's spoke (O, P3) gets the smaller id than T1's
        // incoming spoke (P2, O).
        let e_o_p3 = add_edge(&mut m, (vo, origin), (v3, p3)); // id 0
        let e_p3_p4 = add_edge(&mut m, (v3, p3), (v4, p4)); // id 1
        let e_p4_o = add_edge(&mut m, (v4, p4), (vo, origin)); // id 2
        let e_o_p1 = add_edge(&mut m, (vo, origin), (v1, p1)); // id 3
        let e_p1_p2 = add_edge(&mut m, (v1, p1), (v2, p2)); // id 4
        let e_p2_o = add_edge(&mut m, (v2, p2), (vo, origin)); // id 5

        let mut graph = IntersectionGraph::new();
        for &eid in &[e_o_p3, e_p3_p4, e_p4_o, e_o_p1, e_p1_p2, e_p2_o] {
            graph.add_edge(eid, super::super::boolean::EdgeType::Boundary);
        }
        graph.resolve_vertices(&m);

        let arr = build_arrangement(&graph, &m, sid).expect("arrangement builds");
        let surface = m.surfaces.get(sid).expect("surface stored");
        let regions = extract_regions(&arr, &m, surface);

        // The correct arrangement of two triangles sharing one vertex is
        // exactly two interior regions with the triangles' edge sets (the
        // outer CW face is discarded).
        let mut sets: Vec<std::collections::BTreeSet<EdgeId>> = regions
            .iter()
            .map(|r| r.iter().map(|(e, _)| *e).collect())
            .collect();
        sets.sort();
        let t2: std::collections::BTreeSet<EdgeId> =
            [e_o_p3, e_p3_p4, e_p4_o].into_iter().collect();
        let t1: std::collections::BTreeSet<EdgeId> =
            [e_o_p1, e_p1_p2, e_p2_o].into_iter().collect();
        let mut want = vec![t1, t2];
        want.sort();
        assert_eq!(
            sets, want,
            "near-parallel spokes at the shared vertex must extract the two \
             triangles exactly (p2={p2:?}, p3={p3:?}); a cyclically wrong ring \
             order at O wires `next` across the triangles"
        );
    }
}
