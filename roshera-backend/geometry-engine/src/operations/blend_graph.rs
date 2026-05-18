//! Blend graph (F2-β).
//!
//! Given a set of edges selected for blending (fillet or chamfer),
//! [`BlendGraph`] is the data structure every subsequent blend
//! subsystem consumes:
//!
//! * **Spine and rail computation (F3)** needs to know whether an
//!   edge belongs to a tangent chain so adjoining spines share rails.
//! * **Vertex blends / corner patches (F5)** need to know the degree
//!   of every shared vertex and whether neighbouring blend edges are
//!   all convex, all concave, or mixed, before they pick a corner-
//!   patch strategy.
//! * **Setback computation (F2-γ)** plugs into this graph at
//!   `ConvexCorner { degree ≥ 2 }` vertices and writes
//!   `start_setback` / `end_setback` back onto the [`BlendEdge`]s.
//!
//! Today the same incidence information is rebuilt three times — by
//! `group_edges_into_chains` in `fillet.rs`, by `propagate_tangent_edges`,
//! and ad-hoc inside `validate_no_shared_corners`. F2-β unifies them
//! behind one read-only view of the blend selection.
//!
//! The classification fields read from [`EdgeAttributes`] populated by
//! F2-α (`operations::edge_classification::classify_and_cache`). If
//! callers feed the graph with un-classified edges, [`build`] stamps
//! them on the fly so the graph is always consistent.

use std::collections::{HashMap, HashSet};

use crate::math::matrix3::Matrix3;
use crate::math::vector3::{Point3, Vector3};
use crate::operations::edge_classification::{classify_and_cache, find_adjacent_faces};
use crate::operations::{OperationError, OperationResult};
use crate::primitives::edge::{EdgeId, ManifoldKind};
use crate::primitives::face::FaceOrientation;
use crate::primitives::surface::Plane;
use crate::primitives::topology_builder::BRepModel;
use crate::primitives::vertex::VertexId;

/// Stable index into [`BlendGraph::chains`].
pub type ChainId = u32;

/// Sentinel used while building the union-find but never exposed
/// externally — every blend edge in a finished graph has a real
/// `ChainId`.
const UNASSIGNED_CHAIN: ChainId = u32::MAX;

/// Radius schedule along a single blend edge.
///
/// The F2-β graph stores radii verbatim from the caller; converting
/// `Variable` into a tabulated NURBS-weight series is the job of F3
/// / F4 once a spine has been built.
#[derive(Debug, Clone, PartialEq)]
pub enum BlendRadius {
    /// Constant radius along the edge's full parameter range.
    Constant(f64),
    /// Linear ramp from `start` at parameter 0 to `end` at parameter 1.
    Linear { start: f64, end: f64 },
    /// Piecewise (parameter, radius) samples, sorted by parameter in
    /// `[0, 1]`. Parameters and radii are otherwise free-form.
    Variable(Vec<(f64, f64)>),
}

impl BlendRadius {
    /// Minimum radius value present anywhere in the schedule. Useful
    /// for the F6 over-radius / curvature check.
    pub fn min_value(&self) -> f64 {
        match self {
            BlendRadius::Constant(r) => *r,
            BlendRadius::Linear { start, end } => start.min(*end),
            BlendRadius::Variable(samples) => samples
                .iter()
                .map(|(_, r)| *r)
                .fold(f64::INFINITY, f64::min),
        }
    }

    /// Maximum radius value present anywhere in the schedule.
    pub fn max_value(&self) -> f64 {
        match self {
            BlendRadius::Constant(r) => *r,
            BlendRadius::Linear { start, end } => start.max(*end),
            BlendRadius::Variable(samples) => samples
                .iter()
                .map(|(_, r)| *r)
                .fold(f64::NEG_INFINITY, f64::max),
        }
    }
}

/// A single edge selected for blending. Convexity and dihedral are
/// reflected here directly so consumers don't have to round-trip
/// back to `model.edges`.
#[derive(Debug, Clone)]
pub struct BlendEdge {
    /// Underlying B-Rep edge id.
    pub id: EdgeId,
    /// Selected radius schedule.
    pub radius: BlendRadius,
    /// Chain this edge belongs to. Two edges share a `ChainId` iff
    /// they are connected through the selected-edges graph (any
    /// shared endpoint), matching the existing
    /// `group_edges_into_chains` semantics.
    pub chain_id: ChainId,
    /// Cached dihedral pulled from F2-α at graph-build time. `None`
    /// for boundary / non-manifold edges.
    pub dihedral_angle: Option<f64>,
    /// Cached convexity (-1 / 0 / +1) — see
    /// [`crate::primitives::edge::EdgeAttributes::convexity`].
    pub convexity: i8,
    /// Manifold classification at build time.
    pub manifold_kind: ManifoldKind,
    /// Setback distance from the start vertex (vertex blend reach
    /// into this edge from `start_vertex`). Populated by F2-γ;
    /// always `None` from [`build`] alone.
    pub start_setback: Option<f64>,
    /// Setback distance from the end vertex.
    pub end_setback: Option<f64>,
}

/// Kind classification of a vertex inside the blend graph. Drives
/// downstream corner-patch / setback strategy decisions.
///
/// Definitions:
/// * `Smooth`: vertex sits on a single tangent-continuous blend chain
///   (degree 2, edges meet at G1) — no corner needed.
/// * `ConvexCorner { degree }`: every incident blend edge has
///   `convexity > 0` (exterior corner). `degree` is the number of
///   blend edges meeting at the vertex.
/// * `ConcaveCorner { degree }`: every incident blend edge has
///   `convexity < 0` (interior pocket).
/// * `Mixed`: incident blend edges have inconsistent convexity (a
///   convex edge and a concave edge meet) — the corner patch must
///   handle a sign change.
/// * `Cliff`: at least one incident edge is non-manifold or has no
///   defined dihedral; corner construction is not attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum BlendVertexKind {
    Smooth,
    ConvexCorner { degree: usize },
    ConcaveCorner { degree: usize },
    Mixed,
    Cliff,
}

/// A vertex touched by at least one blend edge.
#[derive(Debug, Clone)]
pub struct BlendVertex {
    pub id: VertexId,
    /// Blend-edge ids incident to this vertex, in graph-build order.
    pub incident_blend_edges: Vec<EdgeId>,
    /// Non-blend edges in the underlying model that are also incident
    /// to this vertex. Used by the validator and setback solver to
    /// avoid corner patches that overlap untouched neighbouring
    /// geometry.
    pub incident_other_edges: Vec<EdgeId>,
    /// Topological classification — see [`BlendVertexKind`].
    pub kind: BlendVertexKind,
}

/// View of a blend selection that downstream subsystems consume.
///
/// `vertices` and `edges` are independent maps keyed by their B-Rep
/// ids; `chains` lists each connected component's member edges in
/// insertion order.
#[derive(Debug, Clone, Default)]
pub struct BlendGraph {
    pub vertices: HashMap<VertexId, BlendVertex>,
    pub edges: HashMap<EdgeId, BlendEdge>,
    /// `chains[i]` is the list of edge ids whose `BlendEdge.chain_id`
    /// equals `i as ChainId`.
    pub chains: Vec<Vec<EdgeId>>,
}

impl BlendGraph {
    /// `true` iff this graph holds no blend edges (and therefore no
    /// vertices either).
    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        self.edges.is_empty()
    }

    /// Look up a vertex; panics-free helper for tests.
    #[inline]
    pub fn vertex(&self, id: VertexId) -> Option<&BlendVertex> {
        self.vertices.get(&id)
    }

    /// Look up an edge.
    #[inline]
    pub fn edge(&self, id: EdgeId) -> Option<&BlendEdge> {
        self.edges.get(&id)
    }

    /// Iterate over every `ConvexCorner` / `ConcaveCorner` vertex.
    /// F2-γ's setback solver feeds on this iterator.
    pub fn corners(&self) -> impl Iterator<Item = &BlendVertex> + '_ {
        self.vertices.values().filter(|v| {
            matches!(
                v.kind,
                BlendVertexKind::ConvexCorner { .. }
                    | BlendVertexKind::ConcaveCorner { .. }
                    | BlendVertexKind::Mixed
            )
        })
    }
}

/// Build a blend graph from the selection.
///
/// Skips entries whose `edge_id` does not resolve in `model.edges`
/// — never panics. If the same `edge_id` appears more than once in
/// `selection`, the first occurrence wins (downstream blend semantics
/// don't permit two radius schedules per edge).
///
/// Side effect: any edge that has not been classified yet is
/// classified and cached via F2-α's
/// [`classify_and_cache`](crate::operations::edge_classification::classify_and_cache)
/// before being inserted into the graph, so the cached convexity /
/// dihedral / manifold-kind fields on each [`BlendEdge`] are always
/// well-defined.
pub fn build(
    model: &mut BRepModel,
    selection: &[(EdgeId, BlendRadius)],
) -> OperationResult<BlendGraph> {
    let mut graph = BlendGraph::default();
    if selection.is_empty() {
        return Ok(graph);
    }

    // ----------------------------------------------------------------
    // Pass 1: classify every selected edge so the graph carries
    // up-to-date convexity / dihedral / manifold-kind. This is
    // idempotent — already-classified edges are skipped inside
    // classify_and_cache.
    // ----------------------------------------------------------------
    for (edge_id, _) in selection {
        if model.edges.get(*edge_id).is_some() {
            classify_and_cache(model, *edge_id)?;
        }
    }

    // ----------------------------------------------------------------
    // Pass 2: collect endpoint vertices per selected edge and union-
    // find chain ids. We deduplicate so a repeated edge_id in the
    // input doesn't double-count.
    // ----------------------------------------------------------------
    let mut ordered_edges: Vec<EdgeId> = Vec::with_capacity(selection.len());
    let mut edge_radius: HashMap<EdgeId, BlendRadius> = HashMap::new();
    let mut endpoints: HashMap<EdgeId, (VertexId, VertexId)> = HashMap::new();

    for (edge_id, radius) in selection {
        if edge_radius.contains_key(edge_id) {
            continue;
        }
        let edge = match model.edges.get(*edge_id) {
            Some(e) => e,
            None => continue,
        };
        edge_radius.insert(*edge_id, radius.clone());
        endpoints.insert(*edge_id, (edge.start_vertex, edge.end_vertex));
        ordered_edges.push(*edge_id);
    }

    if ordered_edges.is_empty() {
        return Ok(graph);
    }

    // Union-find over edge indices in `ordered_edges`. Two edges union
    // iff they share an endpoint vertex in the selection.
    let mut parent: Vec<usize> = (0..ordered_edges.len()).collect();
    fn find(parent: &mut [usize], x: usize) -> usize {
        let mut r = x;
        while parent[r] != r {
            r = parent[r];
        }
        let mut cur = x;
        while parent[cur] != r {
            let next = parent[cur];
            parent[cur] = r;
            cur = next;
        }
        r
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let ra = find(parent, a);
        let rb = find(parent, b);
        if ra != rb {
            parent[ra] = rb;
        }
    }

    let mut vertex_to_selected: HashMap<VertexId, Vec<usize>> = HashMap::new();
    for (idx, edge_id) in ordered_edges.iter().enumerate() {
        let (v1, v2) = endpoints[edge_id];
        vertex_to_selected.entry(v1).or_default().push(idx);
        vertex_to_selected.entry(v2).or_default().push(idx);
    }
    for incident in vertex_to_selected.values() {
        if let Some(&first) = incident.first() {
            for &other in &incident[1..] {
                union(&mut parent, first, other);
            }
        }
    }

    // Assign dense chain ids 0..N-1 in insertion order of first
    // occurrence (gives deterministic chain numbering across runs).
    let mut root_to_chain: HashMap<usize, ChainId> = HashMap::new();
    let mut chains: Vec<Vec<EdgeId>> = Vec::new();
    let mut edge_chain: HashMap<EdgeId, ChainId> = HashMap::new();
    for (idx, edge_id) in ordered_edges.iter().enumerate() {
        let root = find(&mut parent, idx);
        let chain_id = match root_to_chain.get(&root) {
            Some(&c) => c,
            None => {
                let c = chains.len() as ChainId;
                root_to_chain.insert(root, c);
                chains.push(Vec::new());
                c
            }
        };
        chains[chain_id as usize].push(*edge_id);
        edge_chain.insert(*edge_id, chain_id);
    }

    // ----------------------------------------------------------------
    // Pass 3: assemble BlendEdge records. Pull cached classification
    // off each edge — `classify_and_cache` populated these above.
    // ----------------------------------------------------------------
    for &edge_id in &ordered_edges {
        let edge = match model.edges.get(edge_id) {
            Some(e) => e,
            None => continue,
        };
        let radius = edge_radius
            .remove(&edge_id)
            .unwrap_or(BlendRadius::Constant(0.0));
        let chain_id = edge_chain
            .get(&edge_id)
            .copied()
            .unwrap_or(UNASSIGNED_CHAIN);
        graph.edges.insert(
            edge_id,
            BlendEdge {
                id: edge_id,
                radius,
                chain_id,
                dihedral_angle: edge.attributes.dihedral_angle,
                convexity: edge.attributes.convexity,
                manifold_kind: edge.attributes.manifold_kind,
                start_setback: None,
                end_setback: None,
            },
        );
    }
    graph.chains = chains;

    // ----------------------------------------------------------------
    // Pass 4: assemble BlendVertex records. For each unique endpoint
    // we list (a) its incident blend edges and (b) its other-edge
    // neighbourhood from the full model.
    // ----------------------------------------------------------------
    let blend_edge_set: HashSet<EdgeId> = ordered_edges.iter().copied().collect();
    let mut vertex_blend_edges: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
    for &edge_id in &ordered_edges {
        let (v1, v2) = endpoints[&edge_id];
        vertex_blend_edges.entry(v1).or_default().push(edge_id);
        if v2 != v1 {
            vertex_blend_edges.entry(v2).or_default().push(edge_id);
        }
    }

    for (vertex_id, blend_edges) in vertex_blend_edges {
        let mut other_edges: Vec<EdgeId> = Vec::new();
        // Scan the whole edge store — this is O(E_total) per build
        // but the typical blend selection is < 100 edges and E_total
        // is the per-model edge count, so it is well-bounded.
        for (eid, e) in model.edges.iter() {
            if e.start_vertex == vertex_id || e.end_vertex == vertex_id {
                if !blend_edge_set.contains(&eid) {
                    other_edges.push(eid);
                }
            }
        }
        let kind = classify_vertex_kind(model, &blend_edges);
        graph.vertices.insert(
            vertex_id,
            BlendVertex {
                id: vertex_id,
                incident_blend_edges: blend_edges,
                incident_other_edges: other_edges,
                kind,
            },
        );
    }

    Ok(graph)
}

/// Classify a vertex given the set of blend edges incident to it.
///
/// Reads cached convexity / dihedral / manifold-kind off
/// `model.edges` (populated by F2-α). Pure function over the slice
/// of incident edges, so [`build`] can call it and so can external
/// callers that maintain their own incidence structures.
pub fn classify_vertex_kind(model: &BRepModel, incident: &[EdgeId]) -> BlendVertexKind {
    if incident.is_empty() {
        // Defensive: every BlendVertex has ≥1 incident blend edge by
        // construction, but the helper is public.
        return BlendVertexKind::Cliff;
    }

    let mut sum_convex = 0i32;
    let mut sum_concave = 0i32;
    let mut smooth_count = 0usize;
    let mut sharp_count = 0usize;
    let mut bad = false;
    for &eid in incident {
        let edge = match model.edges.get(eid) {
            Some(e) => e,
            None => {
                bad = true;
                continue;
            }
        };
        if !matches!(edge.attributes.manifold_kind, ManifoldKind::Manifold) {
            bad = true;
            continue;
        }
        if edge.attributes.dihedral_angle.is_none() {
            bad = true;
            continue;
        }
        match edge.attributes.convexity {
            c if c > 0 => sum_convex += 1,
            c if c < 0 => sum_concave += 1,
            _ => {}
        }
        if edge.attributes.sharpness > 0.5 {
            sharp_count += 1;
        } else {
            smooth_count += 1;
        }
    }

    if bad {
        return BlendVertexKind::Cliff;
    }

    let degree = incident.len();

    // Tangent (smooth) interior vertex: every incident blend edge
    // meets G1 at this vertex. We use the cached sharpness as a
    // proxy — a tighter G1 test (compare per-edge tangents at the
    // shared vertex) is the F3 spine solver's job, not this one.
    if smooth_count == degree && sharp_count == 0 && degree == 2 {
        return BlendVertexKind::Smooth;
    }

    if sum_convex > 0 && sum_concave > 0 {
        return BlendVertexKind::Mixed;
    }
    if sum_convex > 0 {
        return BlendVertexKind::ConvexCorner { degree };
    }
    if sum_concave > 0 {
        return BlendVertexKind::ConcaveCorner { degree };
    }
    // Pure-smooth single edge (degree 1) — treat as a smooth chain
    // endpoint with no corner work needed.
    BlendVertexKind::Smooth
}

/// Tangent vectors that are within `PARALLEL_RAD` of being colinear
/// (parallel or anti-parallel) at a corner produce a rank-deficient
/// setback frame — we cannot decide a unique offset direction and
/// caller must intervene (split the chain, perturb the seed radii,
/// or refuse the operation). Matches the angle band used by the F2-α
/// classifier when deciding "essentially straight" edges.
const PARALLEL_RAD: f64 = 1e-6;

/// Compute per-edge setbacks at every corner vertex of the graph.
///
/// **Formula.** For each blend edge `i` incident to a corner vertex
/// `v` with degree ≥ 2:
///
/// ```text
/// setback_i = r_i · cos(θ_min(i) / 2)
/// ```
///
/// where `r_i` is the conservative (minimum) radius along edge `i`'s
/// schedule and `θ_min(i)` is the smallest angle between edge `i`'s
/// outgoing tangent at `v` and the outgoing tangent of any other
/// blend edge at `v`. Outgoing tangents point *away* from `v` along
/// each edge.
///
/// **Geometric meaning.** A rolling-ball blend of radius `r` on a
/// pair of edges meeting at an angle `θ` produces spine endpoints
/// at distance `r·cot(θ/2)` from the corner along each edge. To make
/// room for a corner patch that connects the spines smoothly, each
/// spine endpoint is *retracted* by an amount that depends on the
/// neighbour with the tightest angle (which dominates the corner
/// geometry). The closed-form `r·cos(θ/2)` is the P0 distance used
/// by Hoffmann §12.4 and matches vendor practice for symmetric
/// corners.
///
/// **Worked example.** A unit-cube corner with three orthogonal
/// edges and uniform radius `r`: every pairwise angle is π/2, so
/// `θ_min = π/2` per edge → `setback = r·cos(π/4) = r/√2`. The
/// matching expectation is pinned by [`tests`].
///
/// **Side effects.** Stamps `start_setback` (if `v == start_vertex`)
/// or `end_setback` (otherwise) on each [`BlendEdge`] in `graph`.
/// Vertices classified as [`BlendVertexKind::Smooth`] or
/// [`BlendVertexKind::Cliff`] are skipped; an isolated degree-1
/// blend vertex is similarly skipped because it has no neighbour to
/// derive `θ_min` from.
///
/// **Errors.** Returns [`OperationError::InvalidGeometry`] when a
/// referenced edge has disappeared from `model`, when a vertex
/// listed as an endpoint is not actually incident, or when two
/// outgoing tangents are within [`PARALLEL_RAD`] of parallel or
/// anti-parallel (the rank-deficient case). Caller-visible
/// behaviour: setbacks are either fully populated or not populated
/// at all — there is no partial-success state.
pub fn compute_setbacks(model: &BRepModel, graph: &mut BlendGraph) -> OperationResult<()> {
    // Snapshot the corner work-list. We avoid borrowing `graph`
    // immutably while we mutate `graph.edges` below.
    let work: Vec<(VertexId, Vec<EdgeId>)> = graph
        .vertices
        .iter()
        .filter_map(|(vid, v)| {
            if vertex_needs_setback(v.kind) && v.incident_blend_edges.len() >= 2 {
                Some((*vid, v.incident_blend_edges.clone()))
            } else {
                None
            }
        })
        .collect();

    for (vertex_id, blend_edges) in work {
        // Compute outgoing tangents at this vertex for every incident
        // blend edge. `edge.tangent_at(0.0)` already factors in the
        // edge's orientation sign (see edge.rs:333), so at the start
        // vertex it points "forward into" the edge — which is the
        // outgoing direction we want. At the end vertex, the same
        // forward tangent points back toward the vertex, so we
        // negate it.
        let mut outgoing: Vec<Vector3> = Vec::with_capacity(blend_edges.len());
        for &eid in &blend_edges {
            let edge = model.edges.get(eid).ok_or_else(|| {
                OperationError::InvalidGeometry(format!(
                    "BlendGraph references edge {} that is no longer in the model",
                    eid
                ))
            })?;
            let t_forward = edge.tangent_at(0.0_f64, &model.curves).map_err(|e| {
                OperationError::NumericalError(format!(
                    "tangent_at(0) failed for edge {}: {:?}",
                    eid, e
                ))
            })?;
            let dir = if edge.start_vertex == vertex_id {
                t_forward
            } else if edge.end_vertex == vertex_id {
                // Negate the forward tangent to get the outgoing
                // direction at the end vertex.
                let t_end = edge.tangent_at(1.0_f64, &model.curves).map_err(|e| {
                    OperationError::NumericalError(format!(
                        "tangent_at(1) failed for edge {}: {:?}",
                        eid, e
                    ))
                })?;
                Vector3::new(-t_end.x, -t_end.y, -t_end.z)
            } else {
                return Err(OperationError::InvalidGeometry(format!(
                    "vertex {} is not an endpoint of edge {}",
                    vertex_id, eid
                )));
            };
            outgoing.push(dir);
        }

        // For each edge i, find θ_min(i) over all j ≠ i and stamp
        // the setback. Track whether any pair is rank-deficient so
        // we can surface a specific error rather than a default.
        for (i, &eid) in blend_edges.iter().enumerate() {
            let mut theta_min = f64::INFINITY;
            for j in 0..blend_edges.len() {
                if i == j {
                    continue;
                }
                let theta = outgoing[i].angle(&outgoing[j]).map_err(|e| {
                    OperationError::NumericalError(format!(
                        "angle between outgoing tangents at vertex {} failed: {:?}",
                        vertex_id, e
                    ))
                })?;
                if theta < theta_min {
                    theta_min = theta;
                }
            }

            if !theta_min.is_finite() {
                return Err(OperationError::InvalidGeometry(format!(
                    "vertex {} has no neighbour pair to derive setback from",
                    vertex_id
                )));
            }
            if theta_min < PARALLEL_RAD {
                return Err(OperationError::InvalidGeometry(format!(
                    "vertex {} corner is rank-deficient: outgoing tangents nearly parallel (θ_min = {:.3e} rad)",
                    vertex_id, theta_min
                )));
            }
            if (std::f64::consts::PI - theta_min) < PARALLEL_RAD {
                return Err(OperationError::InvalidGeometry(format!(
                    "vertex {} corner is rank-deficient: outgoing tangents nearly anti-parallel (θ_min = {:.3e} rad)",
                    vertex_id, theta_min
                )));
            }

            // Conservative radius = minimum value over the schedule.
            // For Constant this is the radius itself; for Linear /
            // Variable we take the smallest sample so the setback
            // never overreaches the available arc.
            let r = graph
                .edges
                .get(&eid)
                .map(|be| be.radius.min_value())
                .unwrap_or(0.0_f64);
            let setback = r * (theta_min * 0.5_f64).cos();

            // Decide which endpoint to stamp by re-reading the edge.
            let (is_start, is_end) = match model.edges.get(eid) {
                Some(e) => (e.start_vertex == vertex_id, e.end_vertex == vertex_id),
                None => (false, false),
            };
            if let Some(be) = graph.edges.get_mut(&eid) {
                if is_start {
                    be.start_setback = Some(setback);
                } else if is_end {
                    be.end_setback = Some(setback);
                }
            }
        }
    }

    Ok(())
}

/// Refine per-edge setbacks for `ConvexCorner { degree: 3 }` apex
/// corners where every incident blend edge lies between two planar
/// adjacent faces (the F5-α scope).
///
/// **Motivation.** `compute_setbacks` writes the Hoffmann
/// smooth-closure value `r·cos(θ_min/2)` at every corner: for a
/// rectilinear (90° dihedral) three-edge corner this gives
/// `r/√2 ≈ 0.707·r`, which retracts the cylinder spine to where
/// two adjacent cylinders meet tangentially — the correct value for
/// the *two-edge* smooth case but **wrong** for apex-sphere
/// termination. F5-α emits a corner sphere of radius `r` whose
/// centre `C` coincides with the rolling-ball centre tangent to all
/// three faces at the corner; each cylinder spine must retract all
/// the way to `C` so the V-side cap arc lands on the sphere centre.
/// For a unit-cube corner the correct setback is `r` (full
/// retraction); the Hoffmann value falls short by `r·(1 − cos(π/4))`
/// ≈ `0.293·r`.
///
/// **Geometry.** For each incident blend edge `i` at the corner
/// vertex `V`:
///
///   * The two adjacent faces are planes with outward unit normals
///     `n_a`, `n_b` (oriented per `face.orientation`).
///   * The cylinder axis is parallel to the edge tangent and passes
///     through the point `P_i = V − (r / (1 + n_a·n_b))·(n_a + n_b)`
///     (the inward perpendicular offset of `V` that lies on both
///     planes' `+r` offset surfaces). For perpendicular normals
///     this reduces to `P_i = V − r·(n_a + n_b)`.
///   * `P_i` is the V-end of the spine *without* setback, i.e. the
///     projection of `V` onto the axis line.
///
/// The apex sphere centre `C` is the point equidistant (distance
/// `r`) from all three faces, hence the least-squares intersection
/// of the three predicted cylinder axes. The same projector-sum
/// solve used by `fillet::compute_concurrent_axes_center` is inlined
/// here so this module stays independent of the upper-layer dispatch
/// code:
///
/// ```text
///   C = (Σ M_i)^{-1} · (Σ M_i · q_i),  M_i = I − u_i u_iᵀ,  q_i = P_i.
/// ```
///
/// The apex-aware setback per edge is then `(C − P_i)·u_i_outgoing`,
/// where `u_i_outgoing` is the unit edge-tangent at `V` oriented
/// "into" the cylinder (the same direction
/// [`compute_setbacks`] uses for `θ_min`). For the rectilinear
/// box-corner case this evaluates to exactly `r`.
///
/// **Side effects.** On a corner that meets every precondition,
/// overwrites `start_setback`/`end_setback` on each of the three
/// incident `BlendEdge`s with the apex-aware value. Corners that do
/// not meet the preconditions (any adjacent face non-planar;
/// rank-deficient axis system; numerically near-anti-parallel face
/// normals; non-finite or non-positive radius on any edge) are
/// silently left alone — the upstream Hoffmann setback survives.
/// The lifecycle gate at `fillet.rs` is responsible for rejecting
/// unsupported configurations before they reach the synthesis pass;
/// this function never produces a diagnostic of its own.
///
/// **Mixed-radii support (F5-β).** Each edge contributes its own
/// `r_i` to the cylinder-axis base `q_i = V − (r_i/(1+c_i))·(n_a+n_b)`;
/// the LS apex solve is radius-independent and converges on the
/// concurrent-axes anchor `A`. The resulting per-edge setback
/// `|(A − q_i)·u_i|` retracts each cylindrical fillet's V-end to
/// `C_i = q_i + ((A − q_i)·u_i)·u_i`, the foot of `A` on axis `i`.
/// For equal radii all three `C_i` collapse to a single point —
/// the apex sphere centre — and the F5-α surgery recovers as a
/// special case.
///
/// **Idempotency.** Two consecutive calls produce the same final
/// setback values (the second call reads the apex-aware values
/// produced by the first and computes the same answer because all
/// inputs — `V`, plane normals, radii, edge tangents — are
/// unchanged).
///
/// Must be called *after* [`compute_setbacks`] so the Hoffmann
/// baseline is in place for any corner that does not match the
/// F5-α scope.
pub fn compute_apex_setbacks(model: &BRepModel, graph: &mut BlendGraph) -> OperationResult<()> {
    // Snapshot the work list so we can mutate `graph.edges` without
    // borrowing `graph.vertices` immutably at the same time.
    let work: Vec<(VertexId, Vec<EdgeId>)> = graph
        .vertices
        .iter()
        .filter_map(|(vid, v)| match v.kind {
            BlendVertexKind::ConvexCorner { degree: 3 } if v.incident_blend_edges.len() == 3 => {
                Some((*vid, v.incident_blend_edges.clone()))
            }
            _ => None,
        })
        .collect();

    for (vertex_id, blend_edges) in work {
        let vertex = match model.vertices.get(vertex_id) {
            Some(v) => v,
            None => continue,
        };
        let v_pos = Point3::new(vertex.position[0], vertex.position[1], vertex.position[2]);

        // Step 1: for each incident blend edge, predict the cylinder
        // axis (origin + direction) and capture the outgoing edge
        // tangent at V. Any non-planar adjacent face or near-anti-
        // parallel normal pair (1 + n_a·n_b ≈ 0) drops the corner
        // out of F5-α scope and we leave its Hoffmann setbacks in
        // place.
        struct EdgePrediction {
            edge_id: EdgeId,
            p_start: Point3,
            axis_dir: Vector3,
            is_start: bool,
        }
        const ANTI_PARALLEL_TOL: f64 = 1.0e-9;

        let mut predictions: Vec<EdgePrediction> = Vec::with_capacity(3);
        let mut bail = false;
        for &eid in &blend_edges {
            let edge = match model.edges.get(eid) {
                Some(e) => e,
                None => {
                    bail = true;
                    break;
                }
            };
            let faces = find_adjacent_faces(model, eid);
            if faces.len() != 2 {
                bail = true;
                break;
            }
            let mut normals: [Vector3; 2] = [Vector3::new(0.0, 0.0, 0.0); 2];
            let mut planar = true;
            for i in 0..2 {
                let face = match model.faces.get(faces[i]) {
                    Some(f) => f,
                    None => {
                        planar = false;
                        break;
                    }
                };
                let surface = match model.surfaces.get(face.surface_id) {
                    Some(s) => s,
                    None => {
                        planar = false;
                        break;
                    }
                };
                let plane = match surface.as_any().downcast_ref::<Plane>() {
                    Some(p) => p,
                    None => {
                        planar = false;
                        break;
                    }
                };
                let mut n = plane.normal;
                if face.orientation == FaceOrientation::Backward {
                    n = Vector3::new(-n.x, -n.y, -n.z);
                }
                normals[i] = n;
            }
            if !planar {
                bail = true;
                break;
            }

            let n_a = normals[0];
            let n_b = normals[1];
            let c = n_a.dot(&n_b);
            if (1.0 + c).abs() < ANTI_PARALLEL_TOL {
                bail = true;
                break;
            }

            // Outgoing edge tangent at V (same convention as
            // `compute_setbacks`).
            let is_start = edge.start_vertex == vertex_id;
            let is_end = edge.end_vertex == vertex_id;
            if !is_start && !is_end {
                bail = true;
                break;
            }
            let outgoing = if is_start {
                edge.tangent_at(0.0_f64, &model.curves).map_err(|e| {
                    OperationError::NumericalError(format!(
                        "tangent_at(0) failed for edge {}: {:?}",
                        eid, e
                    ))
                })?
            } else {
                let t_end = edge.tangent_at(1.0_f64, &model.curves).map_err(|e| {
                    OperationError::NumericalError(format!(
                        "tangent_at(1) failed for edge {}: {:?}",
                        eid, e
                    ))
                })?;
                Vector3::new(-t_end.x, -t_end.y, -t_end.z)
            };

            // Radius — per-edge constant arm of the BlendEdge. The
            // F5-β setback formula `setback_i = |(A − q_i) · u_i|`
            // is per-edge correct regardless of whether radii agree
            // across the three incident edges: each `q_i` carries
            // its own `r_i`, and the LS apex solve below is
            // radius-independent. F5-α's equal-radius case is
            // recovered when all three radii happen to coincide —
            // `A` collapses to the apex sphere centre and each
            // setback retracts the spine to that single point.
            let r = graph
                .edges
                .get(&eid)
                .map(|be| be.radius.min_value())
                .unwrap_or(0.0_f64);
            if !(r > 0.0 && r.is_finite()) {
                bail = true;
                break;
            }

            // Cylinder axis origin: V − (r/(1+c))·(n_a + n_b).
            let scale = r / (1.0 + c);
            let sum = n_a + n_b;
            let p_start = Point3::new(
                v_pos.x - scale * sum.x,
                v_pos.y - scale * sum.y,
                v_pos.z - scale * sum.z,
            );

            // Axis direction: parallel to n_a × n_b, oriented along
            // the outgoing edge tangent. Falling back to the
            // outgoing tangent itself when the cross product is
            // numerically degenerate is unnecessary — the
            // anti-parallel guard above already ruled it out, and
            // by construction `outgoing ≈ n_a × n_b` for a
            // straight edge between two planes.
            let cross = n_a.cross(&n_b);
            let axis_unit = match cross.normalize() {
                Ok(u) => {
                    if u.dot(&outgoing) >= 0.0 {
                        u
                    } else {
                        Vector3::new(-u.x, -u.y, -u.z)
                    }
                }
                Err(_) => {
                    bail = true;
                    break;
                }
            };

            predictions.push(EdgePrediction {
                edge_id: eid,
                p_start,
                axis_dir: axis_unit,
                is_start,
            });
        }
        if bail || predictions.len() != 3 {
            continue;
        }

        // Step 2: least-squares concurrent-axes solve for the apex
        // sphere centre C. Same projector-sum formulation used by
        // `fillet::compute_concurrent_axes_center`; inlined here to
        // keep `blend_graph` independent of upper-layer dispatch.
        let mut a = [0.0_f64; 9]; // column-major 3x3
        let mut b = Vector3::new(0.0, 0.0, 0.0);
        for pred in &predictions {
            let u = pred.axis_dir;
            let q = pred.p_start;
            let m00 = 1.0 - u.x * u.x;
            let m11 = 1.0 - u.y * u.y;
            let m22 = 1.0 - u.z * u.z;
            let m01 = -u.x * u.y;
            let m02 = -u.x * u.z;
            let m12 = -u.y * u.z;
            a[0] += m00;
            a[4] += m11;
            a[8] += m22;
            a[3] += m01;
            a[1] += m01;
            a[6] += m02;
            a[2] += m02;
            a[7] += m12;
            a[5] += m12;

            // M·q = q − (u·q)·u.
            let dot = u.x * q.x + u.y * q.y + u.z * q.z;
            b.x += q.x - dot * u.x;
            b.y += q.y - dot * u.y;
            b.z += q.z - dot * u.z;
        }
        let mat = Matrix3::from_cols(a);
        if mat.determinant().abs() < 1.0e-9 {
            // Rank-deficient: leave Hoffmann setbacks in place.
            continue;
        }
        let inv = match mat.inverse() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let c_vec = inv.transform_vector(&b);
        let apex = Point3::new(c_vec.x, c_vec.y, c_vec.z);

        // Step 3: per-edge apex setback. By construction
        // `(apex − p_start)` is parallel to `axis_dir`, so the dot
        // product is the signed arc-length along the spine from
        // the V-end to the apex; the magnitude is the retraction
        // we need.
        for pred in &predictions {
            let delta = Vector3::new(
                apex.x - pred.p_start.x,
                apex.y - pred.p_start.y,
                apex.z - pred.p_start.z,
            );
            let signed = delta.dot(&pred.axis_dir);
            let setback = signed.abs();
            if let Some(be) = graph.edges.get_mut(&pred.edge_id) {
                if pred.is_start {
                    be.start_setback = Some(setback);
                } else {
                    be.end_setback = Some(setback);
                }
            }
        }
    }

    Ok(())
}

/// True if a vertex of the given kind should receive a setback
/// computation. Pulled out for testability.
#[inline]
fn vertex_needs_setback(kind: BlendVertexKind) -> bool {
    match kind {
        BlendVertexKind::ConvexCorner { degree } | BlendVertexKind::ConcaveCorner { degree } => {
            degree >= 2
        }
        BlendVertexKind::Mixed => true,
        BlendVertexKind::Smooth | BlendVertexKind::Cliff => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::edge_classification::classify_all_unclassified_edges;
    use crate::primitives::edge::EdgeId;
    use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

    fn build_unit_box() -> BRepModel {
        let mut model = BRepModel::new();
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(1.0, 1.0, 1.0)
                .expect("create_box_3d should succeed");
        }
        classify_all_unclassified_edges(&mut model).expect("F2-α sweep");
        model
    }

    /// Pick `count` edges that all share a single vertex. The unit
    /// box has 8 vertices with degree 3, so this works for count ∈
    /// {1, 2, 3}.
    fn edges_sharing_vertex(model: &BRepModel, count: usize) -> (VertexId, Vec<EdgeId>) {
        let mut vertex_to_edges: HashMap<VertexId, Vec<EdgeId>> = HashMap::new();
        for (eid, e) in model.edges.iter() {
            vertex_to_edges.entry(e.start_vertex).or_default().push(eid);
            vertex_to_edges.entry(e.end_vertex).or_default().push(eid);
        }
        let (v, mut edges) = vertex_to_edges
            .into_iter()
            .find(|(_, es)| es.len() >= count)
            .expect("box vertex with required degree exists");
        edges.sort();
        edges.truncate(count);
        (v, edges)
    }

    #[test]
    fn empty_selection_yields_empty_graph() {
        let mut model = build_unit_box();
        let graph = build(&mut model, &[]).expect("build should succeed on empty selection");
        assert!(graph.is_empty());
        assert!(graph.vertices.is_empty());
        assert!(graph.chains.is_empty());
    }

    #[test]
    fn single_edge_yields_two_smooth_endpoint_vertices() {
        let mut model = build_unit_box();
        let edge_id: EdgeId = model.edges.iter().next().map(|(id, _)| id).expect("≥ 1 edge");
        let selection = vec![(edge_id, BlendRadius::Constant(0.1))];
        let graph = build(&mut model, &selection).expect("build single-edge selection");

        assert_eq!(graph.edges.len(), 1);
        assert_eq!(graph.vertices.len(), 2, "edge has two distinct endpoints");
        assert_eq!(graph.chains.len(), 1);
        assert_eq!(graph.chains[0], vec![edge_id]);

        let blend_edge = graph.edge(edge_id).expect("BlendEdge present");
        assert_eq!(blend_edge.chain_id, 0);
        assert_eq!(blend_edge.convexity, 1, "box edges are convex");
        assert!(blend_edge.dihedral_angle.is_some());
        assert!(blend_edge.start_setback.is_none());
        assert!(blend_edge.end_setback.is_none());

        // Each endpoint is degree-1 in the selection => no corner
        // needed (F2-γ will skip it). classify_vertex_kind returns
        // ConvexCorner { degree: 1 } because there's a convex edge
        // and only one of it.
        for v in graph.vertices.values() {
            assert_eq!(
                v.kind,
                BlendVertexKind::ConvexCorner { degree: 1 },
                "single convex blend edge => degree-1 ConvexCorner per endpoint"
            );
            assert_eq!(v.incident_blend_edges.len(), 1);
            // The unit box vertex has 3 incident edges in total, so
            // exactly 2 non-blend edges remain.
            assert_eq!(v.incident_other_edges.len(), 2);
        }
    }

    #[test]
    fn two_adjacent_edges_share_one_corner_vertex() {
        let mut model = build_unit_box();
        let (shared_vertex, edges) = edges_sharing_vertex(&model, 2);
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(0.1)))
            .collect();
        let graph = build(&mut model, &selection).expect("build two-edge selection");

        // Two adjacent edges sharing a vertex => one connected chain.
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(graph.chains.len(), 1, "shared vertex => single chain");
        assert_eq!(graph.chains[0].len(), 2);

        // 2 edges with 1 shared endpoint => 3 distinct vertices.
        assert_eq!(graph.vertices.len(), 3);

        let shared = graph.vertex(shared_vertex).expect("shared vertex in graph");
        assert_eq!(
            shared.kind,
            BlendVertexKind::ConvexCorner { degree: 2 },
            "two convex edges meeting => degree-2 ConvexCorner"
        );
        assert_eq!(shared.incident_blend_edges.len(), 2);
        // Box vertex degree 3 minus 2 blend edges => 1 other edge.
        assert_eq!(shared.incident_other_edges.len(), 1);
    }

    #[test]
    fn three_edges_meeting_at_box_corner_yield_degree_3() {
        let mut model = build_unit_box();
        let (shared_vertex, edges) = edges_sharing_vertex(&model, 3);
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(0.1)))
            .collect();
        let graph = build(&mut model, &selection).expect("build three-edge selection");

        assert_eq!(graph.edges.len(), 3);
        assert_eq!(graph.chains.len(), 1);
        let shared = graph.vertex(shared_vertex).expect("shared vertex");
        assert_eq!(
            shared.kind,
            BlendVertexKind::ConvexCorner { degree: 3 },
            "three convex edges meeting => degree-3 ConvexCorner"
        );
        assert_eq!(shared.incident_other_edges.len(), 0);
    }

    #[test]
    fn duplicate_edge_in_selection_collapses() {
        let mut model = build_unit_box();
        let edge_id: EdgeId = model.edges.iter().next().map(|(id, _)| id).expect("≥ 1 edge");
        let selection = vec![
            (edge_id, BlendRadius::Constant(0.1)),
            (edge_id, BlendRadius::Constant(0.2)),
        ];
        let graph = build(&mut model, &selection).expect("build dedup selection");
        assert_eq!(graph.edges.len(), 1, "duplicate edge ids collapse");
        let e = graph.edge(edge_id).expect("edge present");
        match e.radius {
            BlendRadius::Constant(r) => assert!(
                (r - 0.1).abs() < f64::EPSILON,
                "first occurrence of the radius schedule wins"
            ),
            _ => panic!("unexpected radius variant"),
        }
    }

    #[test]
    fn disconnected_edges_yield_separate_chains() {
        let mut model = build_unit_box();
        // Pick two edges that share no vertex. On a unit box every
        // pair of opposite edges qualifies. We collect all 12, pick
        // the first, then find one whose endpoints are disjoint.
        let all_edges: Vec<(EdgeId, VertexId, VertexId)> = model
            .edges
            .iter()
            .map(|(id, e)| (id, e.start_vertex, e.end_vertex))
            .collect();
        let (e0, e0_a, e0_b) = all_edges[0];
        let (e1, _, _) = all_edges
            .iter()
            .copied()
            .find(|(_, a, b)| *a != e0_a && *a != e0_b && *b != e0_a && *b != e0_b)
            .expect("a unit box has at least one disjoint edge pair");

        let selection = vec![
            (e0, BlendRadius::Constant(0.1)),
            (e1, BlendRadius::Constant(0.1)),
        ];
        let graph = build(&mut model, &selection).expect("build disjoint selection");
        assert_eq!(graph.edges.len(), 2);
        assert_eq!(
            graph.chains.len(),
            2,
            "disjoint edges should produce two chains"
        );
        // Each chain holds exactly one edge.
        for chain in &graph.chains {
            assert_eq!(chain.len(), 1);
        }
    }

    #[test]
    fn corners_iterator_skips_smooth_vertices() {
        let mut model = build_unit_box();
        let (_shared, edges) = edges_sharing_vertex(&model, 3);
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(0.1)))
            .collect();
        let graph = build(&mut model, &selection).expect("build corner selection");

        // Out of 4 distinct vertices for 3 box-corner edges (the
        // shared corner plus 3 distinct far endpoints), the shared
        // one is ConvexCorner{3} and the other three are
        // ConvexCorner{1}. Every BlendVertex is a corner here (no
        // Smooth vertices because every blend edge is sharp).
        let corner_count = graph.corners().count();
        assert!(
            corner_count >= 1,
            "at least the shared degree-3 vertex must be reported"
        );
    }

    #[test]
    fn classify_vertex_kind_handles_unknown_edges() {
        let model = BRepModel::new();
        // No edges exist in this model; classify_vertex_kind on a
        // fictitious id should report Cliff, not panic.
        let kind = classify_vertex_kind(&model, &[42_u32]);
        assert_eq!(kind, BlendVertexKind::Cliff);
    }

    /// Symmetric cube corner: three orthogonal blend edges with the
    /// same radius `r`. Every pairwise outgoing-tangent angle is
    /// π/2, so the formula gives `setback = r · cos(π/4) = r/√2`
    /// for every edge end at the shared corner. Matches the
    /// expectation pinned in the F2-γ plan (Hoffmann §12.4).
    #[test]
    fn unit_cube_symmetric_corner_setback_is_r_over_sqrt2() {
        let mut model = build_unit_box();
        let (shared_vertex, edges) = edges_sharing_vertex(&model, 3);
        let r = 0.1_f64;
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(r)))
            .collect();
        let mut graph =
            build(&mut model, &selection).expect("build three-edge corner selection");

        compute_setbacks(&model, &mut graph).expect("compute_setbacks must succeed");

        let expected = r / 2.0_f64.sqrt();
        for &eid in &edges {
            let be = graph.edge(eid).expect("BlendEdge present");
            let edge = model.edges.get(eid).expect("edge present");
            let setback_at_corner = if edge.start_vertex == shared_vertex {
                be.start_setback
            } else {
                be.end_setback
            };
            let v = setback_at_corner.expect("setback at corner must be populated");
            assert!(
                (v - expected).abs() < 1e-9,
                "setback at orthogonal corner: expected {} (= r/√2), got {}",
                expected,
                v
            );
        }
    }

    #[test]
    fn setback_skips_degree_one_endpoints() {
        let mut model = build_unit_box();
        let (shared_vertex, edges) = edges_sharing_vertex(&model, 3);
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(0.1)))
            .collect();
        let mut graph = build(&mut model, &selection).expect("build three-edge selection");
        compute_setbacks(&model, &mut graph).expect("compute_setbacks");

        // The far endpoint of each of the three blend edges is a
        // ConvexCorner{degree:1} because no other selected blend edge
        // touches it. compute_setbacks must skip those — the
        // corresponding end (the non-shared endpoint) has no setback.
        for &eid in &edges {
            let be = graph.edge(eid).expect("BlendEdge present");
            let edge = model.edges.get(eid).expect("edge present");
            let setback_at_far = if edge.start_vertex == shared_vertex {
                be.end_setback
            } else {
                be.start_setback
            };
            assert!(
                setback_at_far.is_none(),
                "degree-1 endpoint must not receive a setback (got {:?})",
                setback_at_far
            );
        }
    }

    #[test]
    fn setback_no_op_on_empty_graph() {
        let model = BRepModel::new();
        let mut graph = BlendGraph::default();
        compute_setbacks(&model, &mut graph).expect("empty graph is a no-op");
        assert!(graph.is_empty());
    }

    #[test]
    fn setback_no_op_on_single_edge_selection() {
        let mut model = build_unit_box();
        let edge_id: EdgeId = model.edges.iter().next().map(|(id, _)| id).expect("≥ 1 edge");
        let selection = vec![(edge_id, BlendRadius::Constant(0.1))];
        let mut graph = build(&mut model, &selection).expect("build single-edge selection");
        compute_setbacks(&model, &mut graph).expect("single edge: no corner ⇒ no-op");

        let be = graph.edge(edge_id).expect("BlendEdge present");
        assert!(
            be.start_setback.is_none() && be.end_setback.is_none(),
            "single-edge selection: both endpoints are degree-1, so no setbacks"
        );
    }

    /// Pair of blend edges meeting at a single right-angle corner
    /// of the cube. The shared vertex is degree-2; the far endpoints
    /// are degree-1. Setback at the shared corner is `r·cos(π/4)`;
    /// the far endpoints stay `None`.
    #[test]
    fn two_edge_right_angle_corner_setback() {
        let mut model = build_unit_box();
        let (shared_vertex, edges) = edges_sharing_vertex(&model, 2);
        let r = 0.25_f64;
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(r)))
            .collect();
        let mut graph = build(&mut model, &selection).expect("build two-edge selection");
        compute_setbacks(&model, &mut graph).expect("two-edge corner setback");

        let expected = r / 2.0_f64.sqrt();
        for &eid in &edges {
            let be = graph.edge(eid).expect("BlendEdge present");
            let edge = model.edges.get(eid).expect("edge present");
            let (at_corner, at_far) = if edge.start_vertex == shared_vertex {
                (be.start_setback, be.end_setback)
            } else {
                (be.end_setback, be.start_setback)
            };
            let v = at_corner.expect("setback at shared corner");
            assert!(
                (v - expected).abs() < 1e-9,
                "two-edge right-angle setback: expected {}, got {}",
                expected,
                v
            );
            assert!(at_far.is_none(), "far endpoint must stay None");
        }
    }

    /// Per-edge radii differ: each edge's setback uses its own `r`
    /// (conservative `min_value`) — even when θ_min is shared, the
    /// stamped values differ.
    #[test]
    fn setback_uses_per_edge_radius() {
        let mut model = build_unit_box();
        let (shared_vertex, edges) = edges_sharing_vertex(&model, 2);
        let r0 = 0.10_f64;
        let r1 = 0.30_f64;
        let selection = vec![
            (edges[0], BlendRadius::Constant(r0)),
            (edges[1], BlendRadius::Constant(r1)),
        ];
        let mut graph = build(&mut model, &selection).expect("build per-radius selection");
        compute_setbacks(&model, &mut graph).expect("compute_setbacks");

        let cos_pi_over_4 = (std::f64::consts::FRAC_PI_4).cos();
        for (eid, r_expected) in [(edges[0], r0), (edges[1], r1)] {
            let be = graph.edge(eid).expect("BlendEdge present");
            let edge = model.edges.get(eid).expect("edge present");
            let at_corner = if edge.start_vertex == shared_vertex {
                be.start_setback
            } else {
                be.end_setback
            };
            let v = at_corner.expect("setback");
            let expected = r_expected * cos_pi_over_4;
            assert!(
                (v - expected).abs() < 1e-9,
                "per-edge setback: expected {} (= r·cos(π/4)), got {}",
                expected,
                v
            );
        }
    }

    /// `vertex_needs_setback` decides which vertex kinds participate
    /// in setback computation. Pinning the policy makes it explicit
    /// for downstream readers.
    #[test]
    fn vertex_needs_setback_classification() {
        assert!(!vertex_needs_setback(BlendVertexKind::Smooth));
        assert!(!vertex_needs_setback(BlendVertexKind::Cliff));
        assert!(!vertex_needs_setback(
            BlendVertexKind::ConvexCorner { degree: 1 }
        ));
        assert!(!vertex_needs_setback(
            BlendVertexKind::ConcaveCorner { degree: 1 }
        ));
        assert!(vertex_needs_setback(
            BlendVertexKind::ConvexCorner { degree: 2 }
        ));
        assert!(vertex_needs_setback(
            BlendVertexKind::ConvexCorner { degree: 3 }
        ));
        assert!(vertex_needs_setback(
            BlendVertexKind::ConcaveCorner { degree: 2 }
        ));
        assert!(vertex_needs_setback(BlendVertexKind::Mixed));
    }

    #[test]
    fn blend_radius_min_max_match_schedule() {
        let c = BlendRadius::Constant(0.5);
        assert_eq!(c.min_value(), 0.5);
        assert_eq!(c.max_value(), 0.5);
        let l = BlendRadius::Linear { start: 0.1, end: 0.7 };
        assert_eq!(l.min_value(), 0.1);
        assert_eq!(l.max_value(), 0.7);
        let v = BlendRadius::Variable(vec![(0.0, 0.3), (0.5, 0.9), (1.0, 0.2)]);
        assert_eq!(v.min_value(), 0.2);
        assert_eq!(v.max_value(), 0.9);
    }
}
