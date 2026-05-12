//! 2D polygon-polygon clipping for coplanar boolean face handling.
//!
//! Computes the geometric intersection of two simple (possibly concave)
//! polygons in the plane and returns the closed boundary loops of the
//! overlap region. This is the foundational primitive for the kernel's
//! coplanar-face imprint-merge pipeline:
//!
//!   * Slice C added a face-overlap *predicate* on the coplanar branch
//!     of `plane_plane_intersection`; truly-overlapping coplanar faces
//!     still bail out with `OperationError::CoplanarFaces`.
//!   * Slice E (this module + its boolean wiring) replaces that bail-out
//!     with a real overlap computation. Two coplanar faces whose outer
//!     loops overlap need cutting curves around the shared region so the
//!     downstream `split_face_by_curves` pipeline can carve each face
//!     into sub-faces; this module produces those cutting curves in 2D.
//!
//! # Algorithm
//!
//! Greiner-Hormann (1998) polygon clipping with explicit detection of
//! degenerate configurations (vertex-on-edge / vertex-on-vertex / shared
//! collinear segment). Proper-crossing configurations — the dominant
//! case for translated coplanar extrusions in real CAD workflows — are
//! handled exactly. Degenerate configurations surface as
//! [`OperationError::InvalidGeometry`] so callers do not silently
//! produce garbage results; a future sub-slice can extend the algorithm
//! with the Hormann-Agathos (2001) perturbation scheme.
//!
//! # Phases
//!
//! 1. **Build two doubly-linked polygon chains.** Each node is either a
//!    polygon vertex or an intersection point inserted during phase 2.
//! 2. **Find proper edge-edge intersections.** For every edge of A
//!    against every edge of B, if the segments cross in their open
//!    interiors, insert one intersection node into each chain at the
//!    parametric position, twinned across the chains.
//! 3. **Reject degeneracies.** Any vertex lying on the other polygon's
//!    edge (within tolerance), shared vertex, or near-collinear edge
//!    pair triggers an error rather than a wrong-answer result.
//! 4. **Label entry/exit.** Classify the first regular vertex of A as
//!    inside/outside B via a winding-number test, then alternate the
//!    label at every intersection. Symmetric for B.
//! 5. **Walk the intersection regions.** Starting from each unvisited
//!    *entry* intersection in A's chain, walk forward in A until the
//!    next intersection, jump to its twin in B, walk forward in B until
//!    the next intersection, jump back — repeat until the start node is
//!    reached again. Each completed walk is one closed loop of A ∩ B.
//!
//! # No-intersection short-circuit
//!
//! When phase 2 finds zero proper intersections, the polygons either
//! miss each other, one fully contains the other, or they are equal
//! (rejected as a degeneracy upstream). Test the first vertex of A
//! against B and vice versa to pick the right branch.
//!
//! # References
//!
//! - Greiner & Hormann (1998). *Efficient Clipping of Arbitrary
//!   Polygons*. ACM Transactions on Graphics 17(2), 71-83.
//! - Hormann & Agathos (2001). *The Point in Polygon Problem for
//!   Arbitrary Polygons*. Computational Geometry 20(3), 131-144.
//! - Foley, van Dam, Feiner, Hughes (1996). *Computer Graphics:
//!   Principles and Practice* (2nd ed.), §3.14 (polygon clipping).

use super::{OperationError, OperationResult};
use crate::math::Tolerance;

/// 2D point in clip-space. Owns its coordinates; intentionally distinct
/// from `math::Point2` to keep this module free of the trim-nurbs
/// dependency.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Point2d {
    pub x: f64,
    pub y: f64,
}

impl Point2d {
    /// Construct a point.
    #[inline]
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Squared 2D distance, useful when an exact distance is not needed.
    #[inline]
    fn distance_sq(self, other: Self) -> f64 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        dx * dx + dy * dy
    }

    /// 2D Euclidean distance.
    #[inline]
    fn distance(self, other: Self) -> f64 {
        self.distance_sq(other).sqrt()
    }
}

/// Result of [`intersect_polygons`].
#[derive(Debug, Clone, Default)]
pub struct PolygonClipResult {
    /// Connected components of `A ∩ B`. Each inner `Vec<Point2d>` is a
    /// closed loop with the implicit edge from the last point back to
    /// the first; the loops are CCW-oriented when the input polygons
    /// are CCW-oriented.
    pub regions: Vec<Vec<Point2d>>,
}

impl PolygonClipResult {
    /// `true` if the two input polygons have no overlap.
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }
}

/// Per-face boundary partition from [`partition_boundaries`].
///
/// Each segment is an open polyline of length two — start and end —
/// representing one sub-segment of an input polygon's boundary that
/// lies strictly inside the opposing polygon. The segments are the
/// cutting curves the boolean pipeline needs to split each coplanar
/// face into "inside the opposing face" and "outside" sub-faces.
#[derive(Debug, Clone, Default)]
pub struct BoundaryPartition {
    /// Sub-segments of `polygon_a`'s edges that lie strictly inside
    /// `polygon_b`. Each `(Point2d, Point2d)` is an open line segment
    /// in input-edge winding order.
    pub a_inside_b: Vec<(Point2d, Point2d)>,
    /// Sub-segments of `polygon_b`'s edges that lie strictly inside
    /// `polygon_a`. Same encoding as `a_inside_b`.
    pub b_inside_a: Vec<(Point2d, Point2d)>,
}

/// Partition each polygon's boundary by the other polygon's interior.
///
/// For two simple polygons that overlap in a proper-crossing
/// configuration, this returns:
/// - the sub-segments of A's edges lying strictly inside B
///   (`a_inside_b`) — these are the cuts that split face B into
///   "interior to A" and "exterior to A" parts;
/// - the sub-segments of B's edges lying strictly inside A
///   (`b_inside_a`) — these split face A.
///
/// For two-square overlap (A = unit square at origin, B = unit square
/// at (0.5, 0.5)): `a_inside_b` is the L-shape `(1, 0.5) → (1, 1) →
/// (0.5, 1)` (segments of A's right and top edges inside B); the
/// symmetric L from B's left and bottom edges populates `b_inside_a`.
///
/// # Errors
///
/// Same contract as [`intersect_polygons`]: degenerate configurations
/// (shared vertex, vertex-on-edge, collinear overlap) return
/// [`OperationError::InvalidGeometry`]; under-three vertices returns
/// [`OperationError::InvalidInput`].
pub fn partition_boundaries(
    polygon_a: &[Point2d],
    polygon_b: &[Point2d],
    tolerance: &Tolerance,
) -> OperationResult<BoundaryPartition> {
    if polygon_a.len() < 3 {
        return Err(OperationError::InvalidInput {
            parameter: "polygon_a".to_string(),
            expected: "at least 3 vertices".to_string(),
            received: format!("{} vertices", polygon_a.len()),
        });
    }
    if polygon_b.len() < 3 {
        return Err(OperationError::InvalidInput {
            parameter: "polygon_b".to_string(),
            expected: "at least 3 vertices".to_string(),
            received: format!("{} vertices", polygon_b.len()),
        });
    }
    let eps = tolerance.distance().max(f64::EPSILON);
    let eps_sq = eps * eps;
    let crossings = find_all_crossings(polygon_a, polygon_b, eps, eps_sq)?;

    let a_inside_b = partition_edges(polygon_a, polygon_b, &crossings, true, eps);
    let b_inside_a = partition_edges(polygon_b, polygon_a, &crossings, false, eps);
    Ok(BoundaryPartition {
        a_inside_b,
        b_inside_a,
    })
}

/// Walk `subject`'s edges, splitting each at every crossing, and emit
/// the sub-segments whose midpoint is inside `tester`.
///
/// `crossings_describe_a` selects which side of each crossing record
/// belongs to `subject`: when `true`, `subject` is `polygon_a` and the
/// crossing's `edge_a` / `alpha_a` apply; when `false`, `subject` is
/// `polygon_b` and `edge_b` / `alpha_b` apply.
fn partition_edges(
    subject: &[Point2d],
    tester: &[Point2d],
    crossings: &[Crossing],
    crossings_describe_a: bool,
    eps: f64,
) -> Vec<(Point2d, Point2d)> {
    let n = subject.len();
    let mut per_edge: Vec<Vec<(f64, Point2d)>> = vec![Vec::new(); n];
    for c in crossings {
        let (edge, alpha) = if crossings_describe_a {
            (c.edge_a, c.alpha_a)
        } else {
            (c.edge_b, c.alpha_b)
        };
        per_edge[edge].push((alpha, c.point));
    }

    let mut output = Vec::new();
    for i in 0..n {
        let p0 = subject[i];
        let p1 = subject[(i + 1) % n];

        let mut hits = per_edge[i].clone();
        hits.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // Build the alpha-ordered list of points along this edge.
        let mut pts: Vec<Point2d> = Vec::with_capacity(hits.len() + 2);
        pts.push(p0);
        for (_, p) in &hits {
            pts.push(*p);
        }
        pts.push(p1);

        for w in pts.windows(2) {
            let s = w[0];
            let e = w[1];
            // Skip degenerate sub-segments (an edge ending at a crossing
            // whose alpha is effectively 0 or 1 — should not happen given
            // the in-open-interval guard in segment_segment_intersection,
            // but defensive).
            if s.distance_sq(e) <= eps * eps {
                continue;
            }
            let mid = Point2d::new(0.5 * (s.x + e.x), 0.5 * (s.y + e.y));
            if point_in_polygon(mid, tester, eps) {
                output.push((s, e));
            }
        }
    }
    output
}

/// Compute the geometric intersection `A ∩ B` of two simple polygons.
///
/// Both inputs are interpreted as closed CCW-oriented polygons with an
/// implicit closing edge between the last and first vertex. Returns the
/// closed boundary loops of the overlap region.
///
/// # Errors
///
/// - [`OperationError::InvalidInput`] when either polygon has fewer
///   than three vertices.
/// - [`OperationError::InvalidGeometry`] when the polygons share a
///   vertex, when a vertex of one polygon lies on an edge of the other
///   within `tolerance`, or when two edges are coincident along a
///   segment. These configurations need the Hormann-Agathos
///   perturbation extension, which is not implemented in this slice.
pub fn intersect_polygons(
    polygon_a: &[Point2d],
    polygon_b: &[Point2d],
    tolerance: &Tolerance,
) -> OperationResult<PolygonClipResult> {
    if polygon_a.len() < 3 {
        return Err(OperationError::InvalidInput {
            parameter: "polygon_a".to_string(),
            expected: "at least 3 vertices".to_string(),
            received: format!("{} vertices", polygon_a.len()),
        });
    }
    if polygon_b.len() < 3 {
        return Err(OperationError::InvalidInput {
            parameter: "polygon_b".to_string(),
            expected: "at least 3 vertices".to_string(),
            received: format!("{} vertices", polygon_b.len()),
        });
    }

    let eps = tolerance.distance().max(f64::EPSILON);
    let eps_sq = eps * eps;

    // ----------------------------------------------------------------
    // Phase 1: build polygon chains as vectors of Node records. We use
    // index-based linkage instead of pointers (Rust ownership) — each
    // node stores `next` and `prev` indices into its owning Vec.
    // ----------------------------------------------------------------
    let mut chain_a = build_chain(polygon_a);
    let mut chain_b = build_chain(polygon_b);

    // ----------------------------------------------------------------
    // Phase 2: enumerate all proper edge-edge intersections, inserting
    // a twinned pair of nodes per crossing. Also detect degeneracies
    // and fail loudly.
    // ----------------------------------------------------------------
    let crossings = find_all_crossings(polygon_a, polygon_b, eps, eps_sq)?;

    // No intersections → containment or disjoint.
    if crossings.is_empty() {
        return short_circuit_no_crossings(polygon_a, polygon_b, eps);
    }

    insert_crossings(&mut chain_a, &mut chain_b, &crossings);

    // ----------------------------------------------------------------
    // Phase 3: label entry/exit alternation in each chain.
    // ----------------------------------------------------------------
    label_entry_exit(&mut chain_a, polygon_b, eps);
    label_entry_exit(&mut chain_b, polygon_a, eps);

    // ----------------------------------------------------------------
    // Phase 4: walk closed loops by alternating between chains at every
    // intersection node, starting from unvisited entry intersections in
    // A's chain.
    // ----------------------------------------------------------------
    let regions = walk_regions(&mut chain_a, &mut chain_b);

    Ok(PolygonClipResult { regions })
}

// =====================================================================
// Chain construction
// =====================================================================

/// One node in a polygon chain. Either an original polygon vertex or an
/// intersection point inserted during crossing detection. Linkage is
/// index-based: `next` and `prev` are indices into the owning chain.
#[derive(Clone, Debug)]
struct Node {
    point: Point2d,
    /// Index of the next node in the chain (CCW polygon order).
    next: usize,
    /// Index of the previous node in the chain.
    prev: usize,
    /// `Some(idx_in_other_chain)` if this is an intersection node; the
    /// referenced node in the other chain is geometrically identical.
    twin: Option<usize>,
    /// `true` if walking *forward* along this chain at this intersection
    /// enters the other polygon. Only meaningful when `twin.is_some()`.
    entry: bool,
    /// Set when this node has been emitted into a result region.
    visited: bool,
}

fn build_chain(polygon: &[Point2d]) -> Vec<Node> {
    let n = polygon.len();
    let mut nodes = Vec::with_capacity(n);
    for (i, &p) in polygon.iter().enumerate() {
        nodes.push(Node {
            point: p,
            next: (i + 1) % n,
            prev: (i + n - 1) % n,
            twin: None,
            entry: false,
            visited: false,
        });
    }
    nodes
}

// =====================================================================
// Phase 2: proper edge-edge intersections + degeneracy detection
// =====================================================================

/// One proper crossing between an edge of A and an edge of B. The
/// crossing falls strictly inside both edges' open intervals — endpoint
/// touches are flagged as degeneracies in [`find_all_crossings`] before
/// this struct is built.
#[derive(Debug, Clone, Copy)]
struct Crossing {
    /// Index of A's edge: vertices `polygon_a[edge_a]` to
    /// `polygon_a[edge_a + 1]` (wrapping).
    edge_a: usize,
    /// Index of B's edge.
    edge_b: usize,
    /// Parametric position along A's edge (0..1).
    alpha_a: f64,
    /// Parametric position along B's edge (0..1).
    alpha_b: f64,
    /// Crossing point.
    point: Point2d,
}

fn find_all_crossings(
    polygon_a: &[Point2d],
    polygon_b: &[Point2d],
    eps: f64,
    eps_sq: f64,
) -> OperationResult<Vec<Crossing>> {
    // Degeneracy guard 1: shared vertices.
    for &va in polygon_a {
        for &vb in polygon_b {
            if va.distance_sq(vb) <= eps_sq {
                return Err(OperationError::InvalidGeometry(format!(
                    "polygon_clip: shared/near-coincident vertex at ({:.6}, {:.6}); the \
                     Greiner-Hormann implementation in this slice rejects degenerate \
                     configurations — extend with Hormann-Agathos perturbation to handle",
                    va.x, va.y
                )));
            }
        }
    }

    let na = polygon_a.len();
    let nb = polygon_b.len();
    let mut crossings = Vec::new();

    for i in 0..na {
        let a0 = polygon_a[i];
        let a1 = polygon_a[(i + 1) % na];

        // Degeneracy guard 2: B's vertices on A's edge.
        for &vb in polygon_b {
            if point_on_segment(vb, a0, a1, eps) {
                return Err(OperationError::InvalidGeometry(format!(
                    "polygon_clip: vertex ({:.6}, {:.6}) of polygon_b lies on an edge of \
                     polygon_a within tolerance {:.3e} — degenerate T-junction not \
                     handled in this slice",
                    vb.x, vb.y, eps
                )));
            }
        }

        for j in 0..nb {
            let b0 = polygon_b[j];
            let b1 = polygon_b[(j + 1) % nb];

            // Degeneracy guard 3: A's vertex on B's edge — symmetric.
            if i == 0 {
                for &va in polygon_a {
                    if point_on_segment(va, b0, b1, eps) {
                        return Err(OperationError::InvalidGeometry(format!(
                            "polygon_clip: vertex ({:.6}, {:.6}) of polygon_a lies on an edge \
                             of polygon_b within tolerance {:.3e} — degenerate T-junction \
                             not handled in this slice",
                            va.x, va.y, eps
                        )));
                    }
                }
            }

            if let Some(crossing) = segment_segment_intersection(a0, a1, b0, b1, eps)? {
                crossings.push(Crossing {
                    edge_a: i,
                    edge_b: j,
                    alpha_a: crossing.alpha_a,
                    alpha_b: crossing.alpha_b,
                    point: crossing.point,
                });
            }
        }
    }

    Ok(crossings)
}

#[derive(Debug, Clone, Copy)]
struct SegmentCrossing {
    alpha_a: f64,
    alpha_b: f64,
    point: Point2d,
}

/// Proper crossing of two open segments. Returns `Ok(None)` when the
/// segments are parallel, miss each other, or only touch at an endpoint
/// (endpoint contact is a degeneracy that must be detected separately
/// — this function returns `Err(InvalidGeometry)` on collinear-overlap
/// because that case cannot be represented as a single point and would
/// silently corrupt the chain).
fn segment_segment_intersection(
    a0: Point2d,
    a1: Point2d,
    b0: Point2d,
    b1: Point2d,
    eps: f64,
) -> OperationResult<Option<SegmentCrossing>> {
    // Direction vectors.
    let rx = a1.x - a0.x;
    let ry = a1.y - a0.y;
    let sx = b1.x - b0.x;
    let sy = b1.y - b0.y;

    // 2D cross product of direction vectors.
    let denom = rx * sy - ry * sx;

    // Collinear / parallel: |denom| ~ 0.
    if denom.abs() <= eps * eps {
        // Check if A's endpoints lie on B's line — that would be a
        // collinear configuration. If not, they're just parallel and
        // never meet.
        let qpx = b0.x - a0.x;
        let qpy = b0.y - a0.y;
        let cross_qp_r = qpx * ry - qpy * rx;
        if cross_qp_r.abs() <= eps * (rx.abs() + ry.abs() + 1.0) {
            // Collinear. Check for overlap by projecting onto r.
            let r_len_sq = rx * rx + ry * ry;
            if r_len_sq > 0.0 {
                let t0 = (qpx * rx + qpy * ry) / r_len_sq;
                let t1 = ((b1.x - a0.x) * rx + (b1.y - a0.y) * ry) / r_len_sq;
                let (lo, hi) = if t0 <= t1 { (t0, t1) } else { (t1, t0) };
                // Strict overlap (not just touch at endpoint).
                if hi > eps && lo < 1.0 - eps {
                    return Err(OperationError::InvalidGeometry(format!(
                        "polygon_clip: collinear-overlapping edges from ({:.6},{:.6})-\
                         ({:.6},{:.6}) and ({:.6},{:.6})-({:.6},{:.6}); shared-segment \
                         degeneracy not handled in this slice",
                        a0.x, a0.y, a1.x, a1.y, b0.x, b0.y, b1.x, b1.y
                    )));
                }
            }
        }
        return Ok(None);
    }

    // Standard line-line parameters (Goldman 1990).
    let qpx = b0.x - a0.x;
    let qpy = b0.y - a0.y;
    let alpha_a = (qpx * sy - qpy * sx) / denom;
    let alpha_b = (qpx * ry - qpy * rx) / denom;

    // Require crossing strictly inside both open intervals. Endpoint
    // touches (alpha == 0 or alpha == 1) are degeneracies handled
    // earlier; if one slips through tolerance rounding, treat as miss.
    if alpha_a <= eps || alpha_a >= 1.0 - eps {
        return Ok(None);
    }
    if alpha_b <= eps || alpha_b >= 1.0 - eps {
        return Ok(None);
    }

    let point = Point2d::new(a0.x + alpha_a * rx, a0.y + alpha_a * ry);
    Ok(Some(SegmentCrossing {
        alpha_a,
        alpha_b,
        point,
    }))
}

/// Tolerance-aware test for a point on a closed segment.
fn point_on_segment(p: Point2d, a: Point2d, b: Point2d, eps: f64) -> bool {
    // Project p onto AB. If foot is between A and B and the
    // perpendicular distance is ≤ eps, the point is on the segment.
    let abx = b.x - a.x;
    let aby = b.y - a.y;
    let len_sq = abx * abx + aby * aby;
    if len_sq <= eps * eps {
        // Degenerate edge — treat as point-point.
        return p.distance(a) <= eps;
    }
    let apx = p.x - a.x;
    let apy = p.y - a.y;
    let t = (apx * abx + apy * aby) / len_sq;
    // Strict interior. Endpoint coincidence is handled by the
    // shared-vertex check, so we only flag a degeneracy when p lies
    // strictly between A and B (not at the endpoints).
    if t <= eps || t >= 1.0 - eps {
        return false;
    }
    let foot_x = a.x + t * abx;
    let foot_y = a.y + t * aby;
    let dx = p.x - foot_x;
    let dy = p.y - foot_y;
    (dx * dx + dy * dy).sqrt() <= eps
}

// =====================================================================
// Crossing insertion into doubly-linked chains
// =====================================================================

fn insert_crossings(chain_a: &mut Vec<Node>, chain_b: &mut Vec<Node>, crossings: &[Crossing]) {
    // For deterministic insertion, group crossings by edge and sort by
    // alpha along that edge. The original chain length equals the input
    // polygon's vertex count, so `edge_a == i` means the edge between
    // node `i` and node `i+1 mod n`.
    let na = chain_a.len();
    let nb = chain_b.len();
    let mut groups_a: Vec<Vec<usize>> = vec![Vec::new(); na];
    let mut groups_b: Vec<Vec<usize>> = vec![Vec::new(); nb];
    for (cidx, c) in crossings.iter().enumerate() {
        groups_a[c.edge_a].push(cidx);
        groups_b[c.edge_b].push(cidx);
    }
    for g in groups_a.iter_mut() {
        g.sort_by(|&i, &j| {
            crossings[i]
                .alpha_a
                .partial_cmp(&crossings[j].alpha_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    for g in groups_b.iter_mut() {
        g.sort_by(|&i, &j| {
            crossings[i]
                .alpha_b
                .partial_cmp(&crossings[j].alpha_b)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    // First, append one node per crossing into each chain; remember the
    // resulting indices so we can wire `twin` once both inserts are
    // done.
    let mut a_indices: Vec<usize> = Vec::with_capacity(crossings.len());
    let mut b_indices: Vec<usize> = Vec::with_capacity(crossings.len());
    for c in crossings {
        let ia = chain_a.len();
        chain_a.push(Node {
            point: c.point,
            next: usize::MAX,
            prev: usize::MAX,
            twin: None,
            entry: false,
            visited: false,
        });
        a_indices.push(ia);

        let ib = chain_b.len();
        chain_b.push(Node {
            point: c.point,
            next: usize::MAX,
            prev: usize::MAX,
            twin: None,
            entry: false,
            visited: false,
        });
        b_indices.push(ib);
    }
    for (idx, _) in crossings.iter().enumerate() {
        chain_a[a_indices[idx]].twin = Some(b_indices[idx]);
        chain_b[b_indices[idx]].twin = Some(a_indices[idx]);
    }

    // Splice each edge's crossing group into the chain in alpha order.
    splice_group(chain_a, &groups_a, &a_indices, na);
    splice_group(chain_b, &groups_b, &b_indices, nb);
}

/// Wire the inserted intersection nodes into a chain so that walking
/// `next` from vertex `i` traverses `i`'s crossings in alpha order
/// before reaching vertex `i+1`.
fn splice_group(
    chain: &mut Vec<Node>,
    groups: &[Vec<usize>],
    crossing_chain_indices: &[usize],
    original_vertex_count: usize,
) {
    for (edge_index, group) in groups.iter().enumerate() {
        if group.is_empty() {
            continue;
        }
        let start = edge_index;
        let end = (edge_index + 1) % original_vertex_count;

        // Build the new ordered list of node indices for this edge:
        // [start_vertex, crossings_in_alpha_order..., end_vertex].
        let mut ordered: Vec<usize> = Vec::with_capacity(group.len() + 2);
        ordered.push(start);
        for &crossing_idx in group {
            ordered.push(crossing_chain_indices[crossing_idx]);
        }
        ordered.push(end);

        // Wire prev/next between successive elements.
        for w in ordered.windows(2) {
            let from = w[0];
            let to = w[1];
            chain[from].next = to;
            chain[to].prev = from;
        }
    }
}

// =====================================================================
// Phase 3: entry/exit labelling
// =====================================================================

fn label_entry_exit(chain: &mut [Node], other_polygon: &[Point2d], eps: f64) {
    if chain.is_empty() {
        return;
    }
    // Find the first regular vertex (index < original_vertex_count). We
    // know index 0 is one such vertex since chains are constructed with
    // the original vertices in positions 0..N before any intersections
    // are appended.
    let start_inside = point_in_polygon(chain[0].point, other_polygon, eps);
    let mut inside = start_inside;
    let mut cursor = 0usize;
    let visited_start = cursor;
    loop {
        if chain[cursor].twin.is_some() {
            // Flip and label: `entry` means walking forward from this
            // intersection enters the other polygon, i.e. transition
            // from outside → inside.
            chain[cursor].entry = !inside;
            inside = !inside;
        }
        let next = chain[cursor].next;
        if next == visited_start {
            break;
        }
        cursor = next;
        if cursor == visited_start {
            break;
        }
    }
}

/// Winding-number point-in-polygon (Hormann & Agathos 2001 §3).
///
/// Robust to vertical edges; treats the polygon as a closed region. Vertices
/// of the polygon itself return ambiguous values, but the shared-vertex
/// guard upstream prevents the labelling phase from sampling such points.
fn point_in_polygon(p: Point2d, polygon: &[Point2d], _eps: f64) -> bool {
    let n = polygon.len();
    let mut winding: i32 = 0;
    for i in 0..n {
        let v0 = polygon[i];
        let v1 = polygon[(i + 1) % n];
        if v0.y <= p.y {
            if v1.y > p.y && is_left(v0, v1, p) > 0.0 {
                winding += 1;
            }
        } else if v1.y <= p.y && is_left(v0, v1, p) < 0.0 {
            winding -= 1;
        }
    }
    winding != 0
}

/// 2D cross product test: > 0 iff `p` is left of segment `a → b`.
#[inline]
fn is_left(a: Point2d, b: Point2d, p: Point2d) -> f64 {
    (b.x - a.x) * (p.y - a.y) - (p.x - a.x) * (b.y - a.y)
}

// =====================================================================
// Phase 4: region walking
// =====================================================================

fn walk_regions(chain_a: &mut [Node], chain_b: &mut [Node]) -> Vec<Vec<Point2d>> {
    let mut regions = Vec::new();
    for start in 0..chain_a.len() {
        if chain_a[start].twin.is_none() || !chain_a[start].entry || chain_a[start].visited {
            continue;
        }
        let region = walk_one_region(chain_a, chain_b, start);
        if region.len() >= 3 {
            regions.push(region);
        }
    }
    regions
}

fn walk_one_region(chain_a: &mut [Node], chain_b: &mut [Node], start: usize) -> Vec<Point2d> {
    let mut region = Vec::new();
    let mut cursor = start;
    let mut on_a = true;
    let max_steps = chain_a.len() + chain_b.len() + 1;
    for _ in 0..max_steps {
        // Emit + mark visited.
        if on_a {
            chain_a[cursor].visited = true;
            region.push(chain_a[cursor].point);
        } else {
            chain_b[cursor].visited = true;
            region.push(chain_b[cursor].point);
        }

        // Advance one step forward along the current chain.
        let next = if on_a {
            chain_a[cursor].next
        } else {
            chain_b[cursor].next
        };
        cursor = next;

        // Hit an intersection? Switch chains via the twin.
        let twin = if on_a {
            chain_a[cursor].twin
        } else {
            chain_b[cursor].twin
        };
        if let Some(twin_idx) = twin {
            // Mark this side visited too — both endpoints of the twin
            // pair belong to the same closed region we just stitched.
            if on_a {
                chain_a[cursor].visited = true;
            } else {
                chain_b[cursor].visited = true;
            }
            on_a = !on_a;
            cursor = twin_idx;

            // Closing condition: returned to the starting node on A's
            // chain. (Compared against `start`, which is the starting
            // index on A.)
            if on_a && cursor == start {
                break;
            }
        }
    }
    region
}

// =====================================================================
// Containment / disjoint short-circuit
// =====================================================================

fn short_circuit_no_crossings(
    polygon_a: &[Point2d],
    polygon_b: &[Point2d],
    eps: f64,
) -> OperationResult<PolygonClipResult> {
    // Either A ⊂ B, B ⊂ A, or disjoint.
    if point_in_polygon(polygon_a[0], polygon_b, eps) {
        return Ok(PolygonClipResult {
            regions: vec![polygon_a.to_vec()],
        });
    }
    if point_in_polygon(polygon_b[0], polygon_a, eps) {
        return Ok(PolygonClipResult {
            regions: vec![polygon_b.to_vec()],
        });
    }
    Ok(PolygonClipResult::default())
}

// =====================================================================
// Tests
// =====================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Tolerance;

    fn tol() -> Tolerance {
        Tolerance::from_distance(1e-6)
    }

    fn p(x: f64, y: f64) -> Point2d {
        Point2d::new(x, y)
    }

    /// Signed area of a closed polygon (CCW = positive).
    fn signed_area(poly: &[Point2d]) -> f64 {
        let n = poly.len();
        let mut sum = 0.0;
        for i in 0..n {
            let p0 = poly[i];
            let p1 = poly[(i + 1) % n];
            sum += (p1.x - p0.x) * (p1.y + p0.y);
        }
        -sum * 0.5
    }

    #[test]
    fn disjoint_polygons_return_empty() {
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(5.0, 5.0), p(6.0, 5.0), p(6.0, 6.0), p(5.0, 6.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert!(result.is_empty(), "disjoint should produce no regions");
    }

    #[test]
    fn fully_contained_a_in_b_returns_a() {
        // 1×1 square inside a 10×10 square.
        let a = vec![p(2.0, 2.0), p(3.0, 2.0), p(3.0, 3.0), p(2.0, 3.0)];
        let b = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let area = signed_area(&result.regions[0]).abs();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "expected area 1.0, got {area:.6}"
        );
    }

    #[test]
    fn fully_contained_b_in_a_returns_b() {
        // Outer square in A, inner square in B (B ⊂ A).
        let a = vec![p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0), p(0.0, 10.0)];
        let b = vec![p(2.0, 2.0), p(3.0, 2.0), p(3.0, 3.0), p(2.0, 3.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let area = signed_area(&result.regions[0]).abs();
        assert!(
            (area - 1.0).abs() < 1e-9,
            "expected area 1.0, got {area:.6}"
        );
    }

    #[test]
    fn proper_crossing_two_squares_overlap() {
        // Two unit squares offset by (0.5, 0.5) — overlap is a
        // 0.5×0.5 square of area 0.25.
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(0.5, 0.5), p(1.5, 0.5), p(1.5, 1.5), p(0.5, 1.5)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(
            result.regions.len(),
            1,
            "two overlapping squares should produce 1 region"
        );
        let area = signed_area(&result.regions[0]).abs();
        assert!(
            (area - 0.25).abs() < 1e-9,
            "expected area 0.25, got {area:.6}"
        );
    }

    #[test]
    fn proper_crossing_triangle_and_square() {
        // Triangle straddling a square: the triangle's apex is outside,
        // its base crosses through the square. Both polygons must be
        // CCW-wound (the algorithm's input contract) — the triangle's
        // vertices are ordered to give a positive signed area.
        let square = vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)];
        let triangle = vec![p(-1.0, 2.0), p(2.0, -2.0), p(5.0, 2.0)];
        let result = intersect_polygons(&square, &triangle, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        // Visual area: pentagon bounded by (0,0)-(4,0)-(4,2)-(0,2)
        // minus the triangle apex... actually let me re-derive.
        // The triangle: (-1,2)-(5,2)-(2,-2). Top edge horizontal at y=2.
        // The intersection with the square (0..4, 0..4):
        // - Top edge y=2 enters at (0,2) and exits at (4,2).
        // - Left edge (-1,2)-(2,-2) enters the square at y=0 → x=1.25, exits ...
        // For a single-region test, we only verify > 0 area and a
        // sane vertex count.
        let area = signed_area(&result.regions[0]).abs();
        assert!(area > 0.0, "expected positive area, got {area:.6}");
        assert!(
            result.regions[0].len() >= 3,
            "overlap polygon must have ≥3 vertices, got {}",
            result.regions[0].len()
        );
    }

    #[test]
    fn rejects_polygon_with_fewer_than_three_vertices() {
        let a = vec![p(0.0, 0.0), p(1.0, 0.0)];
        let b = vec![p(0.0, 0.0), p(1.0, 0.0), p(0.5, 1.0)];
        let result = intersect_polygons(&a, &b, &tol());
        assert!(matches!(result, Err(OperationError::InvalidInput { .. })));
    }

    #[test]
    fn rejects_shared_vertex_degeneracy() {
        // Two triangles sharing the vertex (0,0).
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(0.0, 1.0)];
        let b = vec![p(0.0, 0.0), p(-1.0, 0.0), p(0.0, -1.0)];
        let result = intersect_polygons(&a, &b, &tol());
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn rejects_vertex_on_edge_degeneracy() {
        // Triangle b's vertex sits on a's bottom edge.
        let a = vec![p(0.0, 0.0), p(4.0, 0.0), p(2.0, 4.0)];
        let b = vec![p(2.0, 0.0), p(3.0, -1.0), p(1.0, -1.0)];
        let result = intersect_polygons(&a, &b, &tol());
        assert!(matches!(result, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn partition_two_overlapping_squares_yields_two_l_shaped_cuts() {
        // A = unit square at origin, B = unit square at (0.5, 0.5).
        // A's edges inside B: (1, 0.5)→(1, 1) and (1, 1)→(0.5, 1).
        // B's edges inside A: (0.5, 0.5)→(1, 0.5) and (0.5, 1)→(0.5, 0.5).
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(0.5, 0.5), p(1.5, 0.5), p(1.5, 1.5), p(0.5, 1.5)];
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");
        assert_eq!(
            part.a_inside_b.len(),
            2,
            "expected 2 A-edge sub-segments inside B, got {}",
            part.a_inside_b.len()
        );
        assert_eq!(
            part.b_inside_a.len(),
            2,
            "expected 2 B-edge sub-segments inside A, got {}",
            part.b_inside_a.len()
        );
        // Verify the A-edges-inside-B sub-segments chain at (1, 1).
        let endpoints: Vec<Point2d> = part
            .a_inside_b
            .iter()
            .flat_map(|(s, e)| vec![*s, *e])
            .collect();
        assert!(
            endpoints
                .iter()
                .any(|p| (p.x - 1.0).abs() < 1e-9 && (p.y - 1.0).abs() < 1e-9),
            "expected vertex (1, 1) in a_inside_b endpoint set, got {endpoints:?}"
        );
    }

    #[test]
    fn partition_disjoint_polygons_yields_empty() {
        let a = vec![p(0.0, 0.0), p(1.0, 0.0), p(1.0, 1.0), p(0.0, 1.0)];
        let b = vec![p(5.0, 5.0), p(6.0, 5.0), p(6.0, 6.0), p(5.0, 6.0)];
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");
        assert!(part.a_inside_b.is_empty());
        assert!(part.b_inside_a.is_empty());
    }

    #[test]
    fn partition_segments_have_midpoints_inside_other_polygon() {
        // Property test: every output sub-segment's midpoint must lie
        // inside the opposing polygon.
        let a = vec![p(0.0, 0.0), p(4.0, 0.0), p(4.0, 4.0), p(0.0, 4.0)];
        let b = vec![p(2.0, 2.0), p(6.0, 2.0), p(6.0, 6.0), p(2.0, 6.0)];
        let part = partition_boundaries(&a, &b, &tol()).expect("partition ok");

        let eps = 1e-6;
        for (s, e) in &part.a_inside_b {
            let mid = Point2d::new(0.5 * (s.x + e.x), 0.5 * (s.y + e.y));
            assert!(
                point_in_polygon(mid, &b, eps),
                "a_inside_b segment ({s:?}, {e:?}) midpoint {mid:?} not inside B"
            );
        }
        for (s, e) in &part.b_inside_a {
            let mid = Point2d::new(0.5 * (s.x + e.x), 0.5 * (s.y + e.y));
            assert!(
                point_in_polygon(mid, &a, eps),
                "b_inside_a segment ({s:?}, {e:?}) midpoint {mid:?} not inside A"
            );
        }
    }

    #[test]
    fn boundary_loop_winding_matches_input() {
        // Two overlapping CCW squares — the overlap loop should also be
        // CCW (signed area > 0).
        let a = vec![p(0.0, 0.0), p(2.0, 0.0), p(2.0, 2.0), p(0.0, 2.0)];
        let b = vec![p(1.0, 1.0), p(3.0, 1.0), p(3.0, 3.0), p(1.0, 3.0)];
        let result = intersect_polygons(&a, &b, &tol()).expect("clip ok");
        assert_eq!(result.regions.len(), 1);
        let signed = signed_area(&result.regions[0]);
        assert!(signed > 0.0, "expected CCW overlap, got signed area {signed}");
    }
}
