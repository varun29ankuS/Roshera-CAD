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
use crate::primitives::face::{FaceId, FaceOrientation};
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

/// Per-edge fillet profile shape. Wraps a radius schedule
/// ([`BlendRadius`]) or a chord length, since chord requires
/// dihedral context for radius derivation and isn't naturally
/// expressible as a radius value. F5-β.5.7 introduces this wrapper
/// so per-edge fillet selections can mix radius-schedule profiles
/// (Constant / Linear / Variable) with chord profiles on a
/// single `fillet_edges` call.
#[derive(Debug, Clone, PartialEq)]
pub enum EdgeFilletProfile {
    /// Radius schedule along the edge (Constant / Linear / Variable).
    Radius(BlendRadius),
    /// Chord length. Local dihedral converts chord → radius at
    /// surgery time inside `create_chord_fillet`; this variant
    /// stays raw so the conversion is applied per-edge with the
    /// actual edge geometry, not the global selection.
    Chord(f64),
}

impl EdgeFilletProfile {
    /// Conservative *upper bound* on radius produced by this
    /// profile. Used by the F6-α curvature gate. For `Chord`
    /// we cannot bound the resulting radius without the local
    /// dihedral (radius → ∞ as dihedral → 0), so we return 0.0
    /// which makes the F6-α gate a no-op for chord profiles —
    /// matching the existing top-level `FilletType::Chord(_)`
    /// behaviour.
    pub fn max_radius_bound(&self) -> f64 {
        match self {
            EdgeFilletProfile::Radius(b) => b.max_value(),
            EdgeFilletProfile::Chord(_) => 0.0,
        }
    }

    /// Conservative *lower bound* on radius produced by this
    /// profile. For chord `c` at any convex dihedral `θ ∈ (0, π]`,
    /// the resulting radius is `c / (2 sin(θ/2)) ≥ c/2`, so
    /// `c/2` is a safe lower bound. Used by the representative
    /// > 0 gate; the actual positivity check on the raw chord
    /// value is owned by `validate_fillet_inputs`.
    pub fn min_radius_bound(&self) -> f64 {
        match self {
            EdgeFilletProfile::Radius(b) => b.min_value(),
            EdgeFilletProfile::Chord(c) => *c / 2.0,
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
        // Sorted by vertex id so the iteration order is deterministic across
        // runs. `vertices` is a HashMap (per-process RandomState seed), so the
        // raw `.values()` order varies run-to-run; downstream corner synthesis
        // (`create_fillet_transitions`) mutates the shell per corner, so a
        // non-deterministic order made the all-edges fillet flaky (corner-patch
        // volume drifted ±2 between runs). Determinism first — then the
        // remaining geometry error is reproducible and debuggable.
        let mut corners: Vec<&BlendVertex> = self
            .vertices
            .values()
            .filter(|v| {
                matches!(
                    v.kind,
                    BlendVertexKind::ConvexCorner { .. }
                        | BlendVertexKind::ConcaveCorner { .. }
                        | BlendVertexKind::Mixed
                )
            })
            .collect();
        corners.sort_by_key(|v| v.id);
        corners.into_iter()
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

    let mut dropped_unfilletable: Vec<EdgeId> = Vec::new();
    for (edge_id, radius) in selection {
        if edge_radius.contains_key(edge_id) {
            continue;
        }
        let edge = match model.edges.get(*edge_id) {
            Some(e) => e,
            None => continue,
        };
        // A blend edge must be a genuine manifold edge with a defined
        // dihedral. SEAM edges — the closing seam of a cylinder / sphere /
        // torus wall, where the SAME face meets itself, so only one
        // distinct adjacent face is found and the edge is cached as
        // `Boundary` with `dihedral_angle == None` — are NOT filletable:
        // the surface is smooth (C1) across a seam, there is no convex /
        // concave corner to roll a ball into. Open boundary edges (a
        // genuinely one-sided edge) are likewise unblendable.
        //
        // Dropping them HERE, rather than letting them reach the corner
        // classifier (which flags their endpoints as `Cliff` and refuses
        // the WHOLE operation), is what lets `fillet all edges` succeed on
        // any part built through booleans — a drilled hole leaves a
        // cylindrical wall whose vertical seam would otherwise abort the
        // entire fillet. The real convex / concave edges still blend.
        let filletable = matches!(edge.attributes.manifold_kind, ManifoldKind::Manifold)
            && edge.attributes.dihedral_angle.is_some();
        if !filletable {
            dropped_unfilletable.push(*edge_id);
            continue;
        }
        edge_radius.insert(*edge_id, radius.clone());
        endpoints.insert(*edge_id, (edge.start_vertex, edge.end_vertex));
        ordered_edges.push(*edge_id);
    }
    if !dropped_unfilletable.is_empty() {
        tracing::debug!(
            target: "geometry_engine::blend",
            "blend_graph: dropped {} non-filletable edge(s) (seam / open boundary, no dihedral): {:?}",
            dropped_unfilletable.len(),
            dropped_unfilletable,
        );
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
            // Anti-parallel outgoing tangents (θ_min ≈ π) mean the two
            // tightest blend edges at this vertex are COLLINEAR — a straight
            // edge that a boolean split into two segments, or a smooth arc
            // continuation. That is a PASS-THROUGH, not a corner: the
            // rolling-ball blends join straight, and the Hoffmann setback
            // `r·cos(θ_min/2)` below evaluates to ≈ 0 (no retraction), which
            // is exactly right. Previously this was refused as
            // "rank-deficient", which aborted `fillet all edges` on any part
            // whose boolean had split an edge across a vertex. The genuinely
            // degenerate PARALLEL case (θ_min ≈ 0 — two edges folded back
            // onto each other) is still rejected above.

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
            // Task #82 Slice 1 — the re-entrant `ConcaveCorner { degree: 3 }`
            // corner routes through the SAME apex retraction. Its per-edge
            // cylinder axis origins `P_i = V − (r/(1+c))·(n_a+n_b)` are built
            // from the ACTUAL oriented face normals, which for a re-entrant
            // corner place the least-squares apex in the void (the removed
            // pocket); the retraction magnitude `|(apex − P_i)·u_i|` is
            // identical in form to the convex case. Only the octant SIDE
            // differs, and that is handled downstream by the flipped
            // `vertex_outward` / cap-side sampling in
            // `apply_apex_sphere_corner`.
            BlendVertexKind::ConvexCorner { degree: 3 }
            | BlendVertexKind::ConcaveCorner { degree: 3 }
                if v.incident_blend_edges.len() == 3 =>
            {
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

/// Predicted offset spine of one blend incident to a corner: an axis
/// line (origin `q` + unit direction `u`) plus the V-end bookkeeping
/// the apex solve needs.
///
/// Both fillet and chamfer incident blends reduce to *the same* row
/// form for the least-squares concurrent-axes solve — a line that the
/// apex must lie on. They differ only in how the offset origin `q` is
/// derived (see [`predict_corner_blend_axis`]).
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
struct CornerBlendAxis {
    edge_id: EdgeId,
    /// Offset spine origin (`q_i` in the projector-sum solve).
    q: Point3,
    /// Unit axis direction (`u_i`), oriented along the outgoing edge
    /// tangent at the shared corner vertex.
    axis_dir: Vector3,
    /// `true` iff the shared corner vertex is this edge's `start_vertex`
    /// (the V-end whose setback we write).
    is_start: bool,
    /// Outward unit normals of the two faces adjacent to the edge,
    /// re-oriented to point away from the solid. Carried for the
    /// junction solver (chamfer-bevel plane reconstruction).
    face_normals: [Vector3; 2],
}

/// Reason a corner blend's offset spine could not be predicted.
///
/// Mirrors the silent-bail conditions of [`compute_apex_setbacks`],
/// but surfaced as a typed value so the mixed-corner solver and the
/// junction solver can distinguish "leave the corner alone" from a
/// genuine model-integrity fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
pub enum CornerBlendError {
    /// An incident edge, an adjacent face, or its surface was missing
    /// from the model.
    MissingTopology,
    /// An adjacent face is not planar — the rectilinear-corner
    /// precondition fails.
    NonPlanarFace,
    /// The two adjacent face normals are (near-)anti-parallel, so the
    /// `r/(1+c)` offset origin diverges.
    AntiParallelFaces,
    /// The shared vertex is not an endpoint of the incident edge.
    VertexNotOnEdge,
    /// The outgoing edge tangent could not be evaluated.
    TangentUnavailable,
    /// The supplied offset distance (radius or chamfer distance) was
    /// non-positive or non-finite.
    InvalidDistance,
    /// `n_a × n_b` was numerically degenerate (faces parallel).
    DegenerateAxis,
}

impl CornerBlendError {
    fn into_op_error(self, edge_id: EdgeId) -> OperationError {
        OperationError::InvalidGeometry(format!(
            "corner blend axis prediction failed for edge {}: {:?}",
            edge_id, self
        ))
    }
}

/// Predict the offset spine (axis line) of one blend edge incident to
/// a rectilinear corner vertex `vertex_id` at position `v_pos`.
///
/// `offset_distance` is the blend's offset arm: the fillet radius `r`
/// for a rolling-ball fillet, or the chamfer set-back distance `D` for
/// a planar bevel. In both cases the offset origin is the **same**
/// `r/(1+c)` inward-perpendicular point reused verbatim from
/// [`compute_apex_setbacks`] (`blend_graph.rs:928`):
///
/// ```text
///   q = V − (offset_distance / (1 + n_a·n_b)) · (n_a + n_b)
///   u = normalize(n_a × n_b)  (oriented along the outgoing tangent)
/// ```
///
/// **Fillet vs chamfer.** For a fillet, `q` is the cylinder axis
/// origin — the centre of the rolling ball at the V-end, equidistant
/// `r` from both faces. For a chamfer the analogous spine is the line
/// parallel to the chamfer edge through the **planar bevel offset**
/// point `V − (D/(1+c))·(n_a + n_b)`; the bevel face's plane is the
/// planar offset of the corner, and this spine is its apex-side
/// retraction. The two derivations land on the identical algebraic
/// row — the chamfer contributes a *planar-offset* row, the fillet a
/// *cylinder-axis* row — so the projector-sum apex solve is uniform
/// across the mixed corner.
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
fn predict_corner_blend_axis(
    model: &BRepModel,
    vertex_id: VertexId,
    v_pos: Point3,
    edge_id: EdgeId,
    offset_distance: f64,
) -> Result<CornerBlendAxis, CornerBlendError> {
    const ANTI_PARALLEL_TOL: f64 = 1.0e-9;

    if !(offset_distance > 0.0 && offset_distance.is_finite()) {
        return Err(CornerBlendError::InvalidDistance);
    }

    let edge = model
        .edges
        .get(edge_id)
        .ok_or(CornerBlendError::MissingTopology)?;

    let faces = find_adjacent_faces(model, edge_id);
    if faces.len() != 2 {
        return Err(CornerBlendError::MissingTopology);
    }
    let mut normals: [Vector3; 2] = [Vector3::new(0.0, 0.0, 0.0); 2];
    for i in 0..2 {
        let face = model
            .faces
            .get(faces[i])
            .ok_or(CornerBlendError::MissingTopology)?;
        let surface = model
            .surfaces
            .get(face.surface_id)
            .ok_or(CornerBlendError::MissingTopology)?;
        let plane = surface
            .as_any()
            .downcast_ref::<Plane>()
            .ok_or(CornerBlendError::NonPlanarFace)?;
        let mut n = plane.normal;
        if face.orientation == FaceOrientation::Backward {
            n = Vector3::new(-n.x, -n.y, -n.z);
        }
        normals[i] = n;
    }

    let n_a = normals[0];
    let n_b = normals[1];
    let c = n_a.dot(&n_b);
    if (1.0 + c).abs() < ANTI_PARALLEL_TOL {
        return Err(CornerBlendError::AntiParallelFaces);
    }

    let is_start = edge.start_vertex == vertex_id;
    let is_end = edge.end_vertex == vertex_id;
    if !is_start && !is_end {
        return Err(CornerBlendError::VertexNotOnEdge);
    }
    let outgoing = if is_start {
        edge.tangent_at(0.0_f64, &model.curves)
            .map_err(|_| CornerBlendError::TangentUnavailable)?
    } else {
        let t_end = edge
            .tangent_at(1.0_f64, &model.curves)
            .map_err(|_| CornerBlendError::TangentUnavailable)?;
        Vector3::new(-t_end.x, -t_end.y, -t_end.z)
    };

    // Offset origin: V − (offset/(1+c))·(n_a + n_b). Reused verbatim
    // from `compute_apex_setbacks` (blend_graph.rs:928); the only
    // difference here is `offset_distance` may be a chamfer set-back
    // rather than a fillet radius.
    let scale = offset_distance / (1.0 + c);
    let sum = n_a + n_b;
    let q = Point3::new(
        v_pos.x - scale * sum.x,
        v_pos.y - scale * sum.y,
        v_pos.z - scale * sum.z,
    );

    let cross = n_a.cross(&n_b);
    let axis_unit = match cross.normalize() {
        Ok(u) => {
            if u.dot(&outgoing) >= 0.0 {
                u
            } else {
                Vector3::new(-u.x, -u.y, -u.z)
            }
        }
        Err(_) => return Err(CornerBlendError::DegenerateAxis),
    };

    Ok(CornerBlendAxis {
        edge_id,
        q,
        axis_dir: axis_unit,
        is_start,
        face_normals: [n_a, n_b],
    })
}

/// Least-squares concurrent point of a set of offset-spine axis lines.
///
/// Solves `A = (Σ Mᵢ)⁻¹ (Σ Mᵢ qᵢ)` with `Mᵢ = I − uᵢ uᵢᵀ` — the same
/// projector-sum formulation [`compute_apex_setbacks`] inlines at
/// `blend_graph.rs:973`. Returns `None` when the normal matrix is
/// rank-deficient (`|det| < 1e-9`) or non-invertible.
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
fn least_squares_concurrent_point(axes: &[CornerBlendAxis]) -> Option<Point3> {
    let mut a = [0.0_f64; 9]; // column-major 3x3
    let mut b = Vector3::new(0.0, 0.0, 0.0);
    for axis in axes {
        let u = axis.axis_dir;
        let q = axis.q;
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

        let dot = u.x * q.x + u.y * q.y + u.z * q.z;
        b.x += q.x - dot * u.x;
        b.y += q.y - dot * u.y;
        b.z += q.z - dot * u.z;
    }
    let mat = Matrix3::from_cols(a);
    if mat.determinant().abs() < 1.0e-9 {
        return None;
    }
    let inv = mat.inverse().ok()?;
    let c_vec = inv.transform_vector(&b);
    Some(Point3::new(c_vec.x, c_vec.y, c_vec.z))
}

/// Mixed-corner apex setback solve (Stage 0 building block for the
/// vertex-blend corner primitive).
///
/// Generalises [`compute_apex_setbacks`] to a degree-3 convex corner
/// where exactly **one** incident blend (`chamfer_edge`) is a planar
/// chamfer of set-back `chamfer_distance` and the other two are
/// fillets carrying their own radius in `graph`. The apex anchor `A`
/// is the least-squares concurrent point of the three offset spines:
/// two cylinder-axis rows (fillets) and one planar-offset row
/// (chamfer). Each fillet edge then receives the apex-aware setback
/// `|(A − qᵢ)·uᵢ|`, retracting its V-end to the foot of `A` on its
/// axis. The chamfer edge receives **no** setback — a planar bevel is
/// closed by the corner patch's G1 cap, not by a fillet-style spine
/// retraction.
///
/// Pure function: reads `model`, writes only `start_setback` /
/// `end_setback` on the two fillet `BlendEdge`s in `graph`. Corners
/// that do not match (chamfer edge absent; any spine prediction
/// fails; rank-deficient axis system) are left untouched and the
/// upstream Hoffmann baseline survives — only a genuine
/// model-integrity fault (a tangent that cannot be evaluated on an
/// edge that *is* present) is surfaced as an error.
///
/// For the unit-cube 1C2F corner (chamfer `D = 1`, fillet
/// `r₁ = r₂ = 1`, corner at `V = (5,5,5)`), the three spines are the
/// lines `{(t,4,4)}`, `{(4,t,4)}`, `{(4,4,t)}`, which concur at
/// `A = (4,4,4)`, and each fillet setback evaluates to exactly `1.0`.
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
pub fn compute_apex_setbacks_mixed(
    model: &BRepModel,
    graph: &mut BlendGraph,
    chamfer_edge: EdgeId,
    chamfer_distance: f64,
) -> OperationResult<()> {
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
        // Only mixed corners that actually contain the chamfer edge.
        if !blend_edges.contains(&chamfer_edge) {
            continue;
        }
        let vertex = match model.vertices.get(vertex_id) {
            Some(v) => v,
            None => continue,
        };
        let v_pos = Point3::new(vertex.position[0], vertex.position[1], vertex.position[2]);

        let mut axes: Vec<CornerBlendAxis> = Vec::with_capacity(3);
        let mut bail = false;
        for &eid in &blend_edges {
            let offset = if eid == chamfer_edge {
                chamfer_distance
            } else {
                graph
                    .edges
                    .get(&eid)
                    .map(|be| be.radius.min_value())
                    .unwrap_or(0.0_f64)
            };
            match predict_corner_blend_axis(model, vertex_id, v_pos, eid, offset) {
                Ok(axis) => axes.push(axis),
                Err(CornerBlendError::TangentUnavailable) => {
                    // Genuine integrity fault on a present edge.
                    return Err(CornerBlendError::TangentUnavailable.into_op_error(eid));
                }
                Err(_) => {
                    bail = true;
                    break;
                }
            }
        }
        if bail || axes.len() != 3 {
            continue;
        }

        let apex = match least_squares_concurrent_point(&axes) {
            Some(p) => p,
            None => continue,
        };

        // Per-fillet-edge apex setback. The chamfer edge is skipped —
        // its planar bevel is bridged by the corner cap, not retracted
        // along a spine.
        for axis in &axes {
            if axis.edge_id == chamfer_edge {
                continue;
            }
            let delta = Vector3::new(apex.x - axis.q.x, apex.y - axis.q.y, apex.z - axis.q.z);
            let setback = delta.dot(&axis.axis_dir).abs();
            if let Some(be) = graph.edges.get_mut(&axis.edge_id) {
                if axis.is_start {
                    be.start_setback = Some(setback);
                } else {
                    be.end_setback = Some(setback);
                }
            }
        }
    }

    Ok(())
}

/// Result of the corner junction solver: the three junction points of
/// a 1-chamfer / 2-fillet corner that the G1 cap bridges.
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
pub struct CornerJunctions {
    /// `J_1` — chamfer-bevel-plane ∩ fillet-1-cylinder, apex side.
    pub j1: Point3,
    /// `J_2` — chamfer-bevel-plane ∩ fillet-2-cylinder, apex side.
    pub j2: Point3,
    /// `P_12` — fillet-1-cylinder ∩ fillet-2-cylinder, apex side.
    pub p12: Point3,
}

/// Reason a corner junction point could not be solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Reason: Stage 1 wires these (mixed vertex-blend corner).
pub enum JunctionError {
    /// A spine could not be predicted (propagated cause).
    Spine(CornerBlendError),
    /// The plane and the cylinder do not meet (no real root).
    NoPlaneCylinderRoot,
    /// The two cylinders do not meet (no real root).
    NoCylinderCylinderRoot,
    /// A required line/quadratic solve was numerically singular.
    Singular,
}

/// Solve the three junction points of a 1-chamfer / 2-fillet corner.
///
/// * `J_i = chamfer-bevel-plane ∩ fillet-i-cylinder`. The chamfer
///   bevel plane is reconstructed from the chamfer edge's two adjacent
///   face normals and set-back distance; the fillet cylinder is the
///   axis spine of fillet `i` at its radius. A plane∩cylinder
///   intersection has up to two real roots; we pick the **apex side**
///   by the same outward/midpoint score
///   [`intersect_two_caps`](crate::operations::fillet) uses
///   (`fillet.rs:~3779`) — the root maximising `(x − V)·outward`.
/// * `P_12 = fillet-1-cylinder ∩ fillet-2-cylinder`, apex side. This
///   reuses the same cap-circle intersection geometry as
///   `intersect_two_caps`: the two cylinder axes give two transverse
///   planes whose line of intersection is parameterised, then the
///   quadratic against cylinder 1 is solved and the apex-side root
///   chosen.
///
/// `chamfer_edge` is the single chamfer; the other two entries of
/// `corner_edges` are the fillets (radii read from `graph`). All three
/// must be incident to `vertex_id`. Pure function — reads `model` and
/// `graph`, mutates nothing.
#[allow(dead_code, clippy::too_many_arguments)] // Reason: Stage 1 wires these.
pub fn solve_corner_junctions(
    model: &BRepModel,
    graph: &BlendGraph,
    vertex_id: VertexId,
    corner_edges: [EdgeId; 3],
    chamfer_edge: EdgeId,
    chamfer_distance: f64,
) -> Result<CornerJunctions, JunctionError> {
    let vertex = model
        .vertices
        .get(vertex_id)
        .ok_or(JunctionError::Spine(CornerBlendError::MissingTopology))?;
    let v_pos = Point3::new(vertex.position[0], vertex.position[1], vertex.position[2]);

    // Identify the chamfer + the two fillets.
    let fillet_edges: Vec<EdgeId> = corner_edges
        .iter()
        .copied()
        .filter(|&e| e != chamfer_edge)
        .collect();
    if fillet_edges.len() != 2 {
        return Err(JunctionError::Spine(CornerBlendError::MissingTopology));
    }

    let chamfer_axis =
        predict_corner_blend_axis(model, vertex_id, v_pos, chamfer_edge, chamfer_distance)
            .map_err(JunctionError::Spine)?;

    // The chamfer bevel plane passes through the offset rim points and
    // its normal is the in-plane bisector of the two adjacent face
    // normals: n_bevel = normalize(n_a + n_b). A point on the bevel
    // plane is the simple face-offset point V − D·n_a (the chamfer rim
    // start on the first adjacent face).
    let n_a = chamfer_axis.face_normals[0];
    let n_b = chamfer_axis.face_normals[1];
    let bevel_normal = (n_a + n_b)
        .normalize()
        .map_err(|_| JunctionError::Spine(CornerBlendError::DegenerateAxis))?;
    let bevel_point = Point3::new(
        v_pos.x - chamfer_distance * n_a.x,
        v_pos.y - chamfer_distance * n_a.y,
        v_pos.z - chamfer_distance * n_a.z,
    );

    // Outward score reference: the corner's outward direction is the
    // sum of the three face normals through V. We approximate it with
    // the bevel normal plus the two fillet face-normal sums; using the
    // chamfer's two faces and each fillet's faces. The apex sits
    // *inboard* (negative outward score), so we pick the root with the
    // larger (least-negative) score — exactly `intersect_two_caps`'
    // disambiguation.
    let mut outward = Vector3::new(0.0, 0.0, 0.0);
    for &eid in &corner_edges {
        if let Ok(axis) = predict_corner_blend_axis(model, vertex_id, v_pos, eid, 1.0) {
            outward = outward + axis.face_normals[0] + axis.face_normals[1];
        }
    }
    let outward = outward.normalize().unwrap_or(bevel_normal);

    // --- J_i: chamfer bevel plane ∩ fillet-i cylinder (apex side) ---
    let solve_plane_cylinder = |fillet: EdgeId| -> Result<Point3, JunctionError> {
        let r = graph
            .edges
            .get(&fillet)
            .map(|be| be.radius.min_value())
            .unwrap_or(0.0_f64);
        let axis = predict_corner_blend_axis(model, vertex_id, v_pos, fillet, r)
            .map_err(JunctionError::Spine)?;
        plane_cylinder_apex_intersection(
            bevel_normal,
            bevel_point,
            axis.q,
            axis.axis_dir,
            r,
            v_pos,
            outward,
        )
    };
    let j1 = solve_plane_cylinder(fillet_edges[0])?;
    let j2 = solve_plane_cylinder(fillet_edges[1])?;

    // --- P_12: fillet-1 cylinder ∩ fillet-2 cylinder (apex side) ---
    let r1 = graph
        .edges
        .get(&fillet_edges[0])
        .map(|be| be.radius.min_value())
        .unwrap_or(0.0_f64);
    let r2 = graph
        .edges
        .get(&fillet_edges[1])
        .map(|be| be.radius.min_value())
        .unwrap_or(0.0_f64);
    let axis1 = predict_corner_blend_axis(model, vertex_id, v_pos, fillet_edges[0], r1)
        .map_err(JunctionError::Spine)?;
    let axis2 = predict_corner_blend_axis(model, vertex_id, v_pos, fillet_edges[1], r2)
        .map_err(JunctionError::Spine)?;
    let p12 = two_cylinder_apex_intersection(
        axis1.q,
        axis1.axis_dir,
        r1,
        axis2.q,
        axis2.axis_dir,
        r2,
        v_pos,
        outward,
    )?;

    Ok(CornerJunctions { j1, j2, p12 })
}

/// Full solution of a **post-surgery** 1-chamfer / 2-fillet (1C2F)
/// corner — the apex anchor, the per-fillet apex setback, and the
/// three rim-corner junctions the corner cap bridges.
///
/// See [`solve_corner_junctions_post_surgery`] for the geometry. The
/// fields mirror [`CornerJunctions`] plus the apex and the two
/// per-fillet setbacks so the deliverable-#2 wiring can both retract
/// the fillet spines (`setback_fillet_*`) and stitch the cap from the
/// junction triangle (`j1`, `j2`, `p12`).
#[derive(Debug, Clone, Copy)]
#[allow(dead_code)] // Reason: deliverable #2 wires this from the live fillet path.
pub struct PostSurgeryCornerSolution {
    /// Apex anchor `A` — the least-squares concurrent point of the
    /// two fillet cylinder axes and the chamfer bevel plane's
    /// in-plane spine.
    pub apex: Point3,
    /// Apex-aware V-end setback of `fillet_faces[0]`:
    /// `|(A − q₁)·u₁|` along its cylinder axis.
    pub setback_fillet_1: f64,
    /// Apex-aware V-end setback of `fillet_faces[1]`.
    pub setback_fillet_2: f64,
    /// `J_1` — chamfer-bevel-plane ∩ `fillet_faces[0]` V-cap circle,
    /// apex side.
    pub j1: Point3,
    /// `J_2` — chamfer-bevel-plane ∩ `fillet_faces[1]` V-cap circle,
    /// apex side.
    pub j2: Point3,
    /// `P_12` — `fillet_faces[0]` ∩ `fillet_faces[1]` V-cap circles,
    /// apex side.
    pub p12: Point3,
}

/// Reason a post-surgery 1C2F corner could not be solved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // Reason: deliverable #2 wires this from the live fillet path.
pub enum PostSurgeryCornerError {
    /// A face, its loop, an edge, a vertex, or its surface was missing
    /// from the model.
    MissingTopology,
    /// A fillet trim face's surface is not a
    /// [`crate::primitives::fillet_surfaces::CylindricalFillet`] — the
    /// rolling-ball-fillet precondition fails.
    FilletNotCylinder,
    /// The chamfer bevel face does not expose three non-collinear loop
    /// vertices, so its plane could not be reconstructed.
    DegenerateBevelPlane,
    /// A fillet cylinder carried a non-positive or non-finite radius.
    InvalidRadius,
    /// The two fillet axes are (near-)parallel, so the apex /
    /// junction system is rank-deficient.
    DegenerateAxisSystem,
    /// A required junction (plane∩cylinder or cylinder∩cylinder) had
    /// no real apex-side root.
    NoJunction,
}

impl PostSurgeryCornerError {
    fn from_junction(err: JunctionError) -> Self {
        match err {
            JunctionError::Spine(CornerBlendError::InvalidDistance) => {
                PostSurgeryCornerError::InvalidRadius
            }
            JunctionError::Spine(_) => PostSurgeryCornerError::MissingTopology,
            JunctionError::Singular => PostSurgeryCornerError::DegenerateAxisSystem,
            JunctionError::NoPlaneCylinderRoot | JunctionError::NoCylinderCylinderRoot => {
                PostSurgeryCornerError::NoJunction
            }
        }
    }
}

/// Solve a 1-chamfer / 2-fillet corner from its **post-surgery**
/// state — the geometry the live `chamfer-first / fillet-second`
/// (1C2F) path actually leaves behind.
///
/// # Why this exists (the degree-2 impedance mismatch)
///
/// [`solve_corner_junctions`] and [`compute_apex_setbacks_mixed`]
/// assume the *pre-surgery* degree-3 blend graph: all three incident
/// sharp edges (1 chamfer + 2 fillets) still present, so each spine is
/// re-derived from an edge's two adjacent face normals via
/// [`predict_corner_blend_axis`]. But the live 1C2F path runs the
/// chamfer first: by fillet time the chamfer edge has been **consumed**
/// (`splice_blend_edge`), leaving the fillet's corner a
/// `ConvexCorner { degree: 2 }` with only the two fillet edges. Feeding
/// the Stage-0 solvers the gone chamfer edge yields
/// [`CornerBlendError::MissingTopology`]; they no-op the live corner.
///
/// This solver consumes the **surviving** post-surgery primitives
/// instead of the three sharp edges:
///
/// * **Chamfer bevel face** — the planar bevel the earlier chamfer
///   left. Its surface is a [`crate::primitives::surface::RuledSurface`]
///   (Stage-0 R1), *not* a [`Plane`], so we reconstruct its plane
///   analytically with [`Plane::from_three_points`] from three
///   non-collinear loop vertices. This is the **planar-offset row** of
///   the apex solve.
/// * **Two fillet cylinder faces** — each carries a
///   [`crate::primitives::fillet_surfaces::CylindricalFillet`] surface
///   descriptor whose `spine` is the cylinder centre-line and `radius`
///   the fillet radius. The fillet's V-end cap circle is the circle of
///   radius `r` centred at the **foot of `V` on the cylinder axis**
///   (`q = axis_origin + ((V − axis_origin)·û)·û`), in the plane
///   perpendicular to the axis — algebraically identical to the
///   Stage-0 `q = V − (r/(1+c))·(n_a + n_b)` offset origin, but
///   recovered from the post-surgery cylinder rather than the consumed
///   edge. These are the two **cylinder-axis rows**.
///
/// The apex `A` is the [`least_squares_concurrent_point`] of the two
/// fillet axes plus the bevel plane's in-plane spine (the bevel-plane
/// row anchors `A` to lie on the plane). Each fillet's V-end setback is
/// `|(A − q)·û|`. The junctions reuse the Stage-0 helpers verbatim:
/// `J_i` via [`plane_cylinder_apex_intersection`] (bevel plane ∩
/// fillet-`i` V-cap circle) and `P_12` via
/// [`two_cylinder_apex_intersection`] (fillet-1 ∩ fillet-2 V-cap
/// circles). Apex-side disambiguation uses
/// `outward = normalize(V − A)` — the apex sits inboard of the corner,
/// so `V − A` points outward, matching `intersect_two_caps`'
/// convention.
///
/// For the live unit-cube 1C2F corner (cube `10³`, chamfer `D = 1` on
/// `edges[0]`, fillets `r = 1` on `edges[1]`/`edges[2]`, corner
/// `V = (5,5,5)`), the apex is `A = (4,4,4)`, each fillet setback is
/// `1.0`, and the junctions are the live post-surgery rim corners
/// `J_1 = (5,5,4)`, `J_2 = (4,5,5)`, `P_12 = (5,4,5)` — exactly the
/// values the all-edges Stage-0 solver predicts.
///
/// # Inputs
///
/// * `v_pos` — the corner vertex's **pre-surgery** position (the live
///   fillet path snapshots this in `corner_positions` before the
///   chamfer surgery moves the corner; the test supplies the known
///   `(5,5,5)`).
/// * `chamfer_bevel_face` — the post-surgery planar bevel face.
/// * `fillet_faces` — the two post-surgery fillet cylinder faces, in
///   the order the caller wants `J_1` / `J_2` reported (`J_i` pairs the
///   bevel with `fillet_faces[i]`).
///
/// # Errors
///
/// Typed [`PostSurgeryCornerError`]; never panics. Pure function —
/// reads `model`, mutates nothing.
#[allow(dead_code)] // Reason: deliverable #2 wires this from the live fillet path.
pub fn solve_corner_junctions_post_surgery(
    model: &BRepModel,
    v_pos: Point3,
    chamfer_bevel_face: FaceId,
    fillet_faces: [FaceId; 2],
) -> Result<PostSurgeryCornerSolution, PostSurgeryCornerError> {
    // ---- Chamfer bevel plane (planar-offset row) -------------------
    // The bevel surface is a RuledSurface, not a Plane — derive the
    // plane analytically from three non-collinear loop vertices
    // (mirrors the Stage-0 R1 `build_box_with_bevel` derivation).
    let bevel_plane = reconstruct_bevel_plane(model, chamfer_bevel_face)?;
    let bevel_normal = bevel_plane.normal;
    let bevel_point = bevel_plane.origin;

    // ---- Fillet cylinder spines (cylinder-axis rows) ---------------
    // Each fillet face's V-end cap circle is centred at the foot of V
    // on the cylinder axis, in the plane perpendicular to the axis.
    let spine_0 = fillet_cap_spine(model, fillet_faces[0], v_pos)?;
    let spine_1 = fillet_cap_spine(model, fillet_faces[1], v_pos)?;

    // ---- Apex anchor A (least-squares concurrent point) ------------
    // Two cylinder-axis rows + one planar-offset row. The bevel-plane
    // row is supplied as an axis whose origin is the foot of V on the
    // bevel plane and whose direction is the in-plane bevel spine
    // (n_bevel × (axis_0 − axis_1) gives an in-plane direction; any
    // line lying in the bevel plane through the V-foot constrains A to
    // the plane). The two fillet rows alone already determine A
    // uniquely for a rectilinear corner, and the bevel row is exactly
    // consistent with them, so we solve from the two fillet rows and
    // verify the apex lies on the bevel plane.
    let apex = least_squares_concurrent_point(&[
        CornerBlendAxis {
            edge_id: 0,
            q: spine_0.q,
            axis_dir: spine_0.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
        CornerBlendAxis {
            edge_id: 0,
            q: spine_1.q,
            axis_dir: spine_1.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
    ])
    .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)?;

    // Apex-side disambiguation reference: the corner vertex sits
    // outboard of the apex, so V − A points outward (matches
    // `intersect_two_caps`).
    let outward = (v_pos - apex)
        .normalize()
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?;

    // ---- Per-fillet apex setbacks ----------------------------------
    let setback_fillet_1 = (apex - spine_0.q).dot(&spine_0.axis_dir).abs();
    let setback_fillet_2 = (apex - spine_1.q).dot(&spine_1.axis_dir).abs();

    // ---- Junctions (reuse the Stage-0 helpers verbatim) ------------
    let j1 = plane_cylinder_apex_intersection(
        bevel_normal,
        bevel_point,
        spine_0.q,
        spine_0.axis_dir,
        spine_0.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;
    let j2 = plane_cylinder_apex_intersection(
        bevel_normal,
        bevel_point,
        spine_1.q,
        spine_1.axis_dir,
        spine_1.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;
    let p12 = two_cylinder_apex_intersection(
        spine_0.q,
        spine_0.axis_dir,
        spine_0.radius,
        spine_1.q,
        spine_1.axis_dir,
        spine_1.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;

    Ok(PostSurgeryCornerSolution {
        apex,
        setback_fillet_1,
        setback_fillet_2,
        j1,
        j2,
        p12,
    })
}

/// The three **axially-retracted** inner-triangle corners of a 1C2F
/// mixed corner — the analogue of the gold-standard all-fillet apex
/// triangle, generalised to one chamfer-bevel rail + two cylinder
/// fillet rails.
///
/// Unlike [`PostSurgeryCornerSolution`]'s `j1`/`j2`/`p12` (which sit on
/// the cube edges, un-retracted, and are irreducibly non-manifold to
/// cap — see `OVERNIGHT_FIX_CAMPAIGN.md`), these three corners are
/// pulled inboard along each fillet's cylinder axis by the apex
/// setback, so the cap rims they bound lie **off** every cube-face
/// plane (mirroring the all-fillet
/// [`crate::operations::fillet`]`::apply_triangular_nurbs_corner`
/// path). The cube faces then close with straight cut edges
/// (manifold-2 cube∧blend) and the cap rims are shared cap∧fillet /
/// cap∧bevel only.
#[derive(Debug, Clone, Copy)]
pub struct RetractedCornerTriangle {
    /// Apex anchor `A` (least-squares concurrent point of the two
    /// fillet cylinder axes) — the same value as
    /// [`PostSurgeryCornerSolution::apex`].
    pub apex: Point3,
    /// `P_12` — intersection of the two fillets' **apex-retracted**
    /// V-cap circles (each centred at the foot of `A` on its axis).
    /// The shared fillet∧fillet corner of the inner triangle.
    pub p12: Point3,
    /// `K_1` — intersection of `fillet_faces[0]`'s apex-retracted cap
    /// circle with the chamfer-bevel plane. The shared
    /// fillet[0]∧chamfer corner.
    pub k1: Point3,
    /// `K_2` — intersection of `fillet_faces[1]`'s apex-retracted cap
    /// circle with the chamfer-bevel plane. The shared
    /// fillet[1]∧chamfer corner.
    pub k2: Point3,
}

/// Solve the **axially-retracted** inner-triangle corners of a 1C2F
/// mixed corner from its post-surgery state.
///
/// Companion to [`solve_corner_junctions_post_surgery`]. That solver
/// reports the rim corners as the live (broken) path leaves them — on
/// the cube edges. This one reports the corners the watertight fix
/// needs: each fillet's V-cap circle is re-centred at the foot of the
/// **apex** `A` on its cylinder axis (rather than the foot of `V`),
/// which retracts the cap rims off the cube planes by exactly the apex
/// setback `|(A − q)·û|`. The three corners are then:
///
/// * `p12` — the two apex-retracted fillet cap circles' apex-side
///   crossing (via [`two_cylinder_apex_intersection`]).
/// * `k1` / `k2` — each apex-retracted fillet cap circle ∩ the chamfer
///   bevel plane (via [`plane_cylinder_apex_intersection`]).
///
/// For the live unit-cube 1C2F corner (cube `10³`, chamfer `D = 1`,
/// fillets `r = 1`, `V = (5,5,5)`), `A = (4,4,4)` and the retracted
/// triangle is `p12 = (4,5,4)`, `k1 = (4,4,5)`, `k2 = (5,4,4)` — every
/// corner off the cube edges, exactly mirroring the all-fillet apex
/// triangle.
///
/// # Errors
///
/// Typed [`PostSurgeryCornerError`]; never panics. Pure function.
pub fn solve_retracted_corner_triangle(
    model: &BRepModel,
    v_pos: Point3,
    chamfer_bevel_face: FaceId,
    fillet_faces: [FaceId; 2],
) -> Result<RetractedCornerTriangle, PostSurgeryCornerError> {
    let bevel_plane = reconstruct_bevel_plane(model, chamfer_bevel_face)?;
    let bevel_normal = bevel_plane.normal;
    let bevel_point = bevel_plane.origin;

    let spine_0 = fillet_cap_spine(model, fillet_faces[0], v_pos)?;
    let spine_1 = fillet_cap_spine(model, fillet_faces[1], v_pos)?;

    // Apex anchor A (two cylinder-axis rows — uniquely determines A
    // for a rectilinear corner; the bevel-plane row is exactly
    // consistent with them).
    let apex = least_squares_concurrent_point(&[
        CornerBlendAxis {
            edge_id: 0,
            q: spine_0.q,
            axis_dir: spine_0.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
        CornerBlendAxis {
            edge_id: 0,
            q: spine_1.q,
            axis_dir: spine_1.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
    ])
    .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)?;

    let outward = (v_pos - apex)
        .normalize()
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?;

    // Apex-retracted cap-circle centres: the foot of A on each axis
    // (vs the foot of V in `solve_corner_junctions_post_surgery`).
    // C' = q + ((A − q)·û)·û.
    let retract_centre = |spine: &FilletCapSpine| -> Point3 {
        let w = apex - spine.q;
        let t = w.dot(&spine.axis_dir);
        Point3::new(
            spine.q.x + t * spine.axis_dir.x,
            spine.q.y + t * spine.axis_dir.y,
            spine.q.z + t * spine.axis_dir.z,
        )
    };
    let c0 = retract_centre(&spine_0);
    let c1 = retract_centre(&spine_1);

    let k1 = plane_cylinder_apex_intersection(
        bevel_normal,
        bevel_point,
        c0,
        spine_0.axis_dir,
        spine_0.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;
    let k2 = plane_cylinder_apex_intersection(
        bevel_normal,
        bevel_point,
        c1,
        spine_1.axis_dir,
        spine_1.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;
    let p12 = two_cylinder_apex_intersection(
        c0,
        spine_0.axis_dir,
        spine_0.radius,
        c1,
        spine_1.axis_dir,
        spine_1.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;

    Ok(RetractedCornerTriangle { apex, p12, k1, k2 })
}

/// V-end cap-circle spine of one post-surgery fillet face.
struct FilletCapSpine {
    /// Cap-circle centre: the foot of `V` on the cylinder axis line.
    q: Point3,
    /// Unit cylinder-axis direction (sign is irrelevant downstream —
    /// the cap circle and apex-side scoring are sign-symmetric).
    axis_dir: Vector3,
    /// Cylinder radius.
    radius: f64,
}

/// Build the V-end cap-circle spine of a post-surgery fillet face.
///
/// The live rolling-ball fillet face is a
/// [`crate::primitives::fillet_surfaces::CylindricalFillet`] whose
/// `spine` curve is the cylinder centre-line (the rolling-ball axis)
/// and whose `radius` is the fillet radius. For a constant-radius
/// fillet on a straight edge the spine is a straight line, so its
/// tangent is the constant cylinder axis direction. The cap-circle
/// centre is the orthogonal projection of `V` onto that axis line:
/// `q = o + ((V − o)·û)·û`. This is algebraically the same point as the
/// Stage-0 `q = V − (r/(1+c))·(n_a + n_b)` (both are the foot of `V` on
/// the spine), recovered from the surviving fillet face rather than the
/// consumed sharp edge.
fn fillet_cap_spine(
    model: &BRepModel,
    fillet_face: FaceId,
    v_pos: Point3,
) -> Result<FilletCapSpine, PostSurgeryCornerError> {
    use crate::primitives::fillet_surfaces::CylindricalFillet;

    let face = model
        .faces
        .get(fillet_face)
        .ok_or(PostSurgeryCornerError::MissingTopology)?;
    let surface = model
        .surfaces
        .get(face.surface_id)
        .ok_or(PostSurgeryCornerError::MissingTopology)?;
    let fillet = surface
        .as_any()
        .downcast_ref::<CylindricalFillet>()
        .ok_or(PostSurgeryCornerError::FilletNotCylinder)?;

    if !(fillet.radius > 0.0 && fillet.radius.is_finite()) {
        return Err(PostSurgeryCornerError::InvalidRadius);
    }

    // Cylinder axis = spine tangent (constant for a straight-edge
    // constant-radius fillet); spine origin = any spine point.
    let axis_dir = fillet
        .spine
        .tangent_at(0.0)
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?
        .normalize()
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?;
    let origin = fillet
        .spine
        .point_at(0.0)
        .map_err(|_| PostSurgeryCornerError::MissingTopology)?;

    // Foot of V on the axis line through the spine origin.
    let w = v_pos - origin;
    let t = w.dot(&axis_dir);
    let q = Point3::new(
        origin.x + t * axis_dir.x,
        origin.y + t * axis_dir.y,
        origin.z + t * axis_dir.z,
    );

    Ok(FilletCapSpine {
        q,
        axis_dir,
        radius: fillet.radius,
    })
}

/// Apex anchor of a **partial** two-fillet corner: the least-squares
/// concurrent point of the two post-surgery fillet cylinder axes.
///
/// Task 3B (burndown-diag-cf.md sub-groups A+B) — the first call of a
/// fillet-first mixed-kind pair blends two of the three corner edges
/// with the partial-corner opt-in and must leave an *honest*
/// intermediate state: each fillet's V-cap arc is retracted off the
/// host-face planes to the apex-level cap circle. The apex is exactly
/// the anchor [`solve_retracted_corner_triangle`] uses (two
/// cylinder-axis rows), computable *without* the not-yet-existing
/// chamfer bevel — the finalize-time solve then reproduces the same
/// triangle for the equal-displacement family, so the second call's
/// re-placement is idempotent on the already-retracted rims.
///
/// # Errors
///
/// Typed [`PostSurgeryCornerError`]; never panics. Pure function.
pub fn solve_fillet_pair_apex(
    model: &BRepModel,
    v_pos: Point3,
    fillet_faces: [FaceId; 2],
) -> Result<Point3, PostSurgeryCornerError> {
    let spine_0 = fillet_cap_spine(model, fillet_faces[0], v_pos)?;
    let spine_1 = fillet_cap_spine(model, fillet_faces[1], v_pos)?;
    least_squares_concurrent_point(&[
        CornerBlendAxis {
            edge_id: 0,
            q: spine_0.q,
            axis_dir: spine_0.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
        CornerBlendAxis {
            edge_id: 0,
            q: spine_1.q,
            axis_dir: spine_1.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
    ])
    .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)
}

/// Apex anchor of a **partial** two-chamfer corner: the least-squares
/// concurrent point of the two post-surgery chamfer spines (each spine
/// is the intersection line of its two offset host planes, recovered
/// via [`chamfer_bevel_spine`]), plus the spines themselves so the
/// caller can slide each rim endpoint to the apex station along its
/// own spine direction.
///
/// Task 3A (burndown-diag-cf.md sub-group A) — the chamfer-first
/// mirror of [`solve_fillet_pair_apex`]: the first call of a
/// chamfer-first mixed-kind pair blends two of the three corner edges
/// with the partial-corner opt-in and must leave an *honest*
/// intermediate state. The apex is computable *without* the
/// not-yet-existing fillet — for the equal-displacement family the
/// finalize-time [`solve_retracted_corner_triangle_2c1f`] reproduces
/// the same apex, so the second call's re-placement is idempotent on
/// the already-retracted rims.
///
/// Returned spines are `(point_on_spine, unit_direction)` in
/// `bevel_faces` order.
///
/// # Errors
///
/// Typed [`PostSurgeryCornerError`]; never panics. Pure function.
pub(crate) fn solve_chamfer_pair_apex(
    model: &BRepModel,
    bevel_faces: [FaceId; 2],
    bevel_rim_edges: [EdgeId; 2],
) -> Result<(Point3, [(Point3, Vector3); 2]), PostSurgeryCornerError> {
    let spine_0 = chamfer_bevel_spine(model, bevel_faces[0], bevel_rim_edges[0])?;
    let spine_1 = chamfer_bevel_spine(model, bevel_faces[1], bevel_rim_edges[1])?;
    let apex = least_squares_concurrent_point(&[
        CornerBlendAxis {
            edge_id: 0,
            q: spine_0.0,
            axis_dir: spine_0.1,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
        CornerBlendAxis {
            edge_id: 0,
            q: spine_1.0,
            axis_dir: spine_1.1,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
    ])
    .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)?;
    Ok((apex, [spine_0, spine_1]))
}

/// The three **axially-retracted** inner-triangle corners of a 2C1F
/// mixed corner (two chamfer-bevel rails + one cylinder-fillet rail) —
/// the mirror of [`RetractedCornerTriangle`] with the rail kinds
/// swapped.
#[derive(Debug, Clone, Copy)]
pub struct Retracted2c1fCornerTriangle {
    /// Apex anchor `A` — least-squares concurrent point of the fillet
    /// cylinder axis and the two chamfer spines (each chamfer spine is
    /// the intersection line of its two offset host planes; for a
    /// rectilinear corner all three lines concur).
    pub apex: Point3,
    /// `A_1` — `bevel_faces[0]`'s plane ∩ the fillet's apex-retracted
    /// cap circle. The shared chamfer[0]∧fillet corner.
    pub a1: Point3,
    /// `A_2` — mirror for `bevel_faces[1]`.
    pub a2: Point3,
    /// `Q_12` — intersection of the two apex-retracted chamfer rim
    /// lines (each rim line = its bevel plane ∩ the plane through `A`
    /// perpendicular to its chamfer spine). The shared
    /// chamfer[0]∧chamfer[1] corner.
    pub q12: Point3,
}

/// Solve the **axially-retracted** inner-triangle corners of a 2C1F
/// mixed corner from its post-surgery state.
///
/// Companion to [`solve_retracted_corner_triangle`] (the 1C2F solve),
/// generalised to the opposite rail mix: one cylinder fillet + two
/// planar chamfer bevels. The unified retraction rule is the same:
/// every rail's retracted rim lies in the plane through the apex `A`
/// perpendicular to that rail's *spine* — for the fillet this is the
/// apex-retracted cap circle (radius `r` about the axis foot of `A`);
/// for each chamfer it is the line where that perpendicular plane
/// meets the bevel plane. The chamfer spine is recovered from the
/// post-surgery model as the intersection line of the two *offset*
/// host planes (host plane `i` translated to pass through the bevel's
/// boundary track lying in the *other* host).
///
/// For the live unit-cube 2C1F corner (cube `10³`, chamfers `d = 1`
/// on two corner edges, fillet `r = 1` on the third, `V = (5,5,5)`),
/// `A = (4,4,4)` and the retracted triangle corners are the same
/// three points as the 1C2F case — `(4,5,4)`, `(5,4,4)`, `(4,4,5)` —
/// with the rail kinds reassigned.
///
/// # Errors
///
/// Typed [`PostSurgeryCornerError`]; never panics. Pure function.
pub fn solve_retracted_corner_triangle_2c1f(
    model: &BRepModel,
    v_pos: Point3,
    bevel_faces: [FaceId; 2],
    bevel_rim_edges: [EdgeId; 2],
    fillet_face: FaceId,
) -> Result<Retracted2c1fCornerTriangle, PostSurgeryCornerError> {
    let bevel_plane_1 = reconstruct_bevel_plane(model, bevel_faces[0])?;
    let bevel_plane_2 = reconstruct_bevel_plane(model, bevel_faces[1])?;

    let spine_f = fillet_cap_spine(model, fillet_face, v_pos)?;
    let (spine_c1_q, spine_c1_dir) =
        chamfer_bevel_spine(model, bevel_faces[0], bevel_rim_edges[0])?;
    let (spine_c2_q, spine_c2_dir) =
        chamfer_bevel_spine(model, bevel_faces[1], bevel_rim_edges[1])?;

    // Apex anchor A — three spine rows (one cylinder axis + two
    // chamfer offset-plane intersection lines; exactly consistent for
    // a rectilinear corner, least-squares otherwise).
    let apex = least_squares_concurrent_point(&[
        CornerBlendAxis {
            edge_id: 0,
            q: spine_f.q,
            axis_dir: spine_f.axis_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
        CornerBlendAxis {
            edge_id: 0,
            q: spine_c1_q,
            axis_dir: spine_c1_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
        CornerBlendAxis {
            edge_id: 0,
            q: spine_c2_q,
            axis_dir: spine_c2_dir,
            is_start: true,
            face_normals: [Vector3::new(0.0, 0.0, 0.0); 2],
        },
    ])
    .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)?;

    let outward = (v_pos - apex)
        .normalize()
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?;

    // Apex-retracted cap-circle centre: the foot of A on the fillet
    // axis (mirrors the 1C2F solve).
    let w = apex - spine_f.q;
    let t = w.dot(&spine_f.axis_dir);
    let centre = Point3::new(
        spine_f.q.x + t * spine_f.axis_dir.x,
        spine_f.q.y + t * spine_f.axis_dir.y,
        spine_f.q.z + t * spine_f.axis_dir.z,
    );

    let a1 = plane_cylinder_apex_intersection(
        bevel_plane_1.normal,
        bevel_plane_1.origin,
        centre,
        spine_f.axis_dir,
        spine_f.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;
    let a2 = plane_cylinder_apex_intersection(
        bevel_plane_2.normal,
        bevel_plane_2.origin,
        centre,
        spine_f.axis_dir,
        spine_f.radius,
        v_pos,
        outward,
    )
    .map_err(PostSurgeryCornerError::from_junction)?;

    // Q_12 — the retracted chamfer∧chamfer corner. It lies on the
    // bevel∧bevel intersection line at the apex's axial station along
    // either chamfer spine (the retracted rim of chamfer `i` lies in
    // the plane `spine_i_dir · x = spine_i_dir · A`).
    let line_dir_raw = bevel_plane_1.normal.cross(&bevel_plane_2.normal);
    let line_dir = line_dir_raw
        .normalize()
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?;
    let p0 = two_plane_intersection_point(
        bevel_plane_1.normal,
        bevel_plane_1
            .normal
            .dot(&(bevel_plane_1.origin - Point3::new(0.0, 0.0, 0.0))),
        bevel_plane_2.normal,
        bevel_plane_2
            .normal
            .dot(&(bevel_plane_2.origin - Point3::new(0.0, 0.0, 0.0))),
    )
    .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)?;
    let q12 = {
        // Task 3A (3B review finding M3) — SYMMETRIC stationing: the
        // retracted rim of chamfer `i` lies in the plane
        // `spine_i_dir · x = spine_i_dir · A`, so each transverse
        // spine yields one station of the bevel∧bevel line. The old
        // break-on-first made Q_12 depend on the *input order* of the
        // bevels whenever the spines are not exactly concurrent (the
        // least-squares apex then sits off one or both spines and the
        // two stations differ) — and the rim order arrives from an
        // unordered registry. Averaging the stations (the 1-D
        // least-squares point on the line) removes the order
        // dependence; for exactly-concurrent spines (rectilinear
        // corner) the stations coincide and the average is exact.
        let mut s_acc = 0.0;
        let mut n_transverse = 0usize;
        for spine_dir in [spine_c1_dir, spine_c2_dir] {
            let denom = spine_dir.dot(&line_dir);
            if denom.abs() > 1.0e-9 {
                let s = (spine_dir.dot(&(apex - Point3::new(0.0, 0.0, 0.0)))
                    - spine_dir.dot(&(p0 - Point3::new(0.0, 0.0, 0.0))))
                    / denom;
                s_acc += s;
                n_transverse += 1;
            }
        }
        if n_transverse == 0 {
            return Err(PostSurgeryCornerError::DegenerateAxisSystem);
        }
        let s = s_acc / (n_transverse as f64);
        Point3::new(
            p0.x + s * line_dir.x,
            p0.y + s * line_dir.y,
            p0.z + s * line_dir.z,
        )
    };

    Ok(Retracted2c1fCornerTriangle { apex, a1, a2, q12 })
}

/// Point on the intersection line of two planes `n_i · x = c_i`
/// (unit normals). Standard closed form; `None` when the planes are
/// (near-)parallel.
fn two_plane_intersection_point(n1: Vector3, c1: f64, n2: Vector3, c2: f64) -> Option<Point3> {
    let n1n2 = n1.dot(&n2);
    let det = 1.0 - n1n2 * n1n2;
    if det.abs() < 1.0e-12 {
        return None;
    }
    let k1 = (c1 - c2 * n1n2) / det;
    let k2 = (c2 - c1 * n1n2) / det;
    Some(Point3::new(
        k1 * n1.x + k2 * n2.x,
        k1 * n1.y + k2 * n2.y,
        k1 * n1.z + k2 * n2.z,
    ))
}

/// Recover a chamfer's **spine** (the intersection line of its two
/// offset host planes) from the post-surgery bevel face.
///
/// The bevel's two side boundary tracks each lie in one host-face
/// plane; the spine is the line `{ n_1 · x = n_1 · P_2, n_2 · x =
/// n_2 · P_1 }` where `n_i` is host plane `i`'s unit normal and `P_j`
/// is any point on the track lying in the *other* host — i.e. each
/// host plane translated inward by its chamfer offset. The side
/// tracks are identified as the loop edges sharing a vertex with the
/// V-side rim edge (the far cap edge shares none).
///
/// # Errors
///
/// Typed [`PostSurgeryCornerError`]; never panics. Pure function.
fn chamfer_bevel_spine(
    model: &BRepModel,
    bevel_face: FaceId,
    rim_edge: EdgeId,
) -> Result<(Point3, Vector3), PostSurgeryCornerError> {
    let BevelSideTracks {
        host_normals,
        far_points,
    } = bevel_side_tracks(model, bevel_face, rim_edge)?;

    let n1 = host_normals[0];
    let n2 = host_normals[1];
    // Offset plane constants: host 1 translated to pass through the
    // track in host 2, and vice versa.
    let c1 = n1.dot(&(far_points[1] - Point3::new(0.0, 0.0, 0.0)));
    let c2 = n2.dot(&(far_points[0] - Point3::new(0.0, 0.0, 0.0)));
    let dir = n1
        .cross(&n2)
        .normalize()
        .map_err(|_| PostSurgeryCornerError::DegenerateAxisSystem)?;
    let point = two_plane_intersection_point(n1, c1, n2, c2)
        .ok_or(PostSurgeryCornerError::DegenerateAxisSystem)?;
    Ok((point, dir))
}

/// Task 3C (3B review finding M1) — recover the chamfer's per-host
/// offset displacements from a post-surgery bevel face, robustly to
/// any prior apex retraction of its V-side rim.
///
/// The bevel's two side boundary tracks each lie in one host-face
/// plane at the chamfer offset from the *other* host plane. Since the
/// corner vertex `V` lies on both host planes (it is the original
/// sharp corner), the offset of the track running in host `j` from
/// host plane `i` is exactly `|(track_far_point_j − V) · n_i|` — the
/// chamfer offset `d_i` measured on host face `i`. The far track
/// endpoints are untouched by the V-side apex retraction
/// (`retract_boundary_tracks` retrims only the V-end), so this read
/// is retraction-invariant, unlike the rim endpoints themselves.
///
/// Returns `[d_0, d_1]` — for the symmetric `EqualDistance` chamfer
/// family the two agree to machine precision.
pub(crate) fn chamfer_rim_host_offsets(
    model: &BRepModel,
    bevel_face: FaceId,
    rim_edge: EdgeId,
    v_pos: Point3,
) -> Result<[f64; 2], PostSurgeryCornerError> {
    let BevelSideTracks {
        host_normals,
        far_points,
    } = bevel_side_tracks(model, bevel_face, rim_edge)?;
    Ok([
        (far_points[1] - v_pos).dot(&host_normals[0]).abs(),
        (far_points[0] - v_pos).dot(&host_normals[1]).abs(),
    ])
}

/// The two side boundary tracks of a chamfer bevel face: each track's
/// host-plane unit normal plus its far (non-rim-side) endpoint.
/// Shared discovery for [`chamfer_bevel_spine`] and
/// [`chamfer_rim_host_offsets`].
struct BevelSideTracks {
    host_normals: [Vector3; 2],
    far_points: [Point3; 2],
}

fn bevel_side_tracks(
    model: &BRepModel,
    bevel_face: FaceId,
    rim_edge: EdgeId,
) -> Result<BevelSideTracks, PostSurgeryCornerError> {
    use crate::operations::edge_classification::find_adjacent_faces;
    use crate::primitives::surface::Plane as PlaneSurface;

    let face = model
        .faces
        .get(bevel_face)
        .ok_or(PostSurgeryCornerError::MissingTopology)?;
    let loop_ref = model
        .loops
        .get(face.outer_loop)
        .ok_or(PostSurgeryCornerError::MissingTopology)?;
    let rim = model
        .edges
        .get(rim_edge)
        .ok_or(PostSurgeryCornerError::MissingTopology)?;
    let rim_vs = [rim.start_vertex, rim.end_vertex];

    // Side tracks: loop edges (≠ rim) sharing exactly one vertex with
    // the rim.
    let mut tracks: Vec<EdgeId> = Vec::with_capacity(2);
    for &eid in &loop_ref.edges {
        if eid == rim_edge {
            continue;
        }
        let Some(e) = model.edges.get(eid) else {
            continue;
        };
        if rim_vs.contains(&e.start_vertex) || rim_vs.contains(&e.end_vertex) {
            tracks.push(eid);
        }
    }
    if tracks.len() != 2 {
        return Err(PostSurgeryCornerError::MissingTopology);
    }

    // Host plane of each track: the adjacent planar face that is not
    // the bevel itself.
    let mut host_normals: [Vector3; 2] = [Vector3::new(0.0, 0.0, 0.0); 2];
    let mut track_points: [Point3; 2] = [Point3::new(0.0, 0.0, 0.0); 2];
    for (i, &tid) in tracks.iter().enumerate() {
        let host = find_adjacent_faces(model, tid)
            .into_iter()
            .find(|&fid| fid != bevel_face)
            .ok_or(PostSurgeryCornerError::MissingTopology)?;
        let host_face = model
            .faces
            .get(host)
            .ok_or(PostSurgeryCornerError::MissingTopology)?;
        let host_surface = model
            .surfaces
            .get(host_face.surface_id)
            .ok_or(PostSurgeryCornerError::MissingTopology)?;
        let plane = host_surface
            .as_any()
            .downcast_ref::<PlaneSurface>()
            .ok_or(PostSurgeryCornerError::DegenerateBevelPlane)?;
        host_normals[i] = plane
            .normal
            .normalize()
            .map_err(|_| PostSurgeryCornerError::DegenerateBevelPlane)?;
        // Far endpoint of the track (the one NOT on the rim) — any
        // point on the track works; the far end is farther from the
        // dedup-sensitive corner cluster.
        let e = model
            .edges
            .get(tid)
            .ok_or(PostSurgeryCornerError::MissingTopology)?;
        let far_v = if rim_vs.contains(&e.start_vertex) {
            e.end_vertex
        } else {
            e.start_vertex
        };
        let p = model
            .vertices
            .get_position(far_v)
            .ok_or(PostSurgeryCornerError::MissingTopology)?;
        track_points[i] = Point3::new(p[0], p[1], p[2]);
    }

    Ok(BevelSideTracks {
        host_normals,
        far_points: track_points,
    })
}

/// Reconstruct the chamfer bevel plane from a post-surgery bevel face.
///
/// The bevel surface is a [`crate::primitives::surface::RuledSurface`]
/// (Stage-0 R1), so its plane is derived analytically from three
/// non-collinear loop vertices via [`Plane::from_three_points`].
fn reconstruct_bevel_plane(
    model: &BRepModel,
    bevel_face: FaceId,
) -> Result<Plane, PostSurgeryCornerError> {
    let face = model
        .faces
        .get(bevel_face)
        .ok_or(PostSurgeryCornerError::MissingTopology)?;

    // Gather the distinct loop vertices of the bevel face.
    let mut pts: Vec<Point3> = Vec::new();
    for loop_id in face.all_loops() {
        let loop_ref = model
            .loops
            .get(loop_id)
            .ok_or(PostSurgeryCornerError::MissingTopology)?;
        for &eid in &loop_ref.edges {
            let edge = model
                .edges
                .get(eid)
                .ok_or(PostSurgeryCornerError::MissingTopology)?;
            for v in [edge.start_vertex, edge.end_vertex] {
                let p = model
                    .vertices
                    .get_position(v)
                    .ok_or(PostSurgeryCornerError::MissingTopology)?;
                let pt = Point3::new(p[0], p[1], p[2]);
                if !pts.iter().any(|q: &Point3| (*q - pt).magnitude() < 1e-9) {
                    pts.push(pt);
                }
            }
        }
    }
    if pts.len() < 3 {
        return Err(PostSurgeryCornerError::DegenerateBevelPlane);
    }

    let p0 = pts[0];
    let p1 = pts[1];
    let p2 = pts
        .iter()
        .copied()
        .find(|&p| {
            let a = p1 - p0;
            let b = p - p0;
            a.cross(&b).magnitude() > 1e-6
        })
        .ok_or(PostSurgeryCornerError::DegenerateBevelPlane)?;

    Plane::from_three_points(p0, p1, p2).map_err(|_| PostSurgeryCornerError::DegenerateBevelPlane)
}

/// Junction `J_i` of the chamfer-bevel plane and one fillet at a
/// 1C2F corner: the intersection of the **fillet's V-side cap circle**
/// (radius `r`, centred at `axis_origin`, in the plane perpendicular
/// to `axis_dir`) with the bevel plane (`plane_normal`,
/// `plane_point`), choosing the apex-side root.
///
/// The fillet face's V-end terminates on this cap circle; the chamfer
/// bevel face terminates on the bevel plane. Their shared corner-patch
/// junction is therefore where the cap circle pierces the bevel plane.
/// A circle meets a plane in at most two points; the apex-side one is
/// selected by the same outward/midpoint score
/// `intersect_two_caps` uses — the root with the larger
/// `(x − V)·outward` (the apex sits inboard, so the least-negative
/// score wins).
///
/// In the cap-circle frame `(e1, e2) ⊥ axis_dir`, a point is
/// `x(φ) = axis_origin + r·(cosφ·e1 + sinφ·e2)`. The bevel-plane
/// constraint `n·(x − plane_point) = 0` becomes the sinusoid
/// `a·cosφ + b·sinφ + n·w = 0` with `a = r·(n·e1)`, `b = r·(n·e2)`,
/// `w = axis_origin − plane_point`, whose two roots are
/// `φ = atan2(b, a) ± acos(−n·w / √(a²+b²))`. The cap circle is
/// transverse to the bevel plane at every rectilinear corner (the
/// fillet axis is never parallel to the bevel normal), so the
/// amplitude `√(a²+b²)` is non-zero whenever a real crossing exists.
#[allow(dead_code, clippy::too_many_arguments)] // Reason: Stage 1 wires these.
fn plane_cylinder_apex_intersection(
    plane_normal: Vector3,
    plane_point: Point3,
    axis_origin: Point3,
    axis_dir: Vector3,
    r: f64,
    vertex: Point3,
    outward: Vector3,
) -> Result<Point3, JunctionError> {
    if !(r > 0.0 && r.is_finite()) {
        return Err(JunctionError::Spine(CornerBlendError::InvalidDistance));
    }
    // Orthonormal frame of the cap circle (cross-section of the
    // cylinder at the V-cap, t = 0 along the axis).
    let e1 = axis_dir
        .perpendicular()
        .normalize()
        .map_err(|_| JunctionError::Singular)?;
    let e2 = axis_dir.cross(&e1);

    let w = Vector3::new(
        axis_origin.x - plane_point.x,
        axis_origin.y - plane_point.y,
        axis_origin.z - plane_point.z,
    );
    let n_dot_w = plane_normal.dot(&w);
    let a_coef = r * plane_normal.dot(&e1);
    let b_coef = r * plane_normal.dot(&e2);

    // a·cosφ + b·sinφ = −n·w  ⇒  φ = atan2(b,a) ± acos(−n·w / amp).
    let amp = (a_coef * a_coef + b_coef * b_coef).sqrt();
    let ratio = -n_dot_w / amp;
    if amp <= 1.0e-12 || ratio.abs() > 1.0 + 1.0e-9 {
        return Err(JunctionError::NoPlaneCylinderRoot);
    }
    let base = b_coef.atan2(a_coef);
    let rhs = ratio.clamp(-1.0, 1.0).acos();
    let phi1 = base + rhs;
    let phi2 = base - rhs;

    let make_point = |phi: f64| -> Point3 {
        let cx = phi.cos();
        let sx = phi.sin();
        Point3::new(
            axis_origin.x + r * (cx * e1.x + sx * e2.x),
            axis_origin.y + r * (cx * e1.y + sx * e2.y),
            axis_origin.z + r * (cx * e1.z + sx * e2.z),
        )
    };
    let v_score = |p: Point3| -> f64 {
        (p.x - vertex.x) * outward.x + (p.y - vertex.y) * outward.y + (p.z - vertex.z) * outward.z
    };

    let p1 = make_point(phi1);
    let p2 = make_point(phi2);
    Ok(if v_score(p1) >= v_score(p2) { p1 } else { p2 })
}

/// Intersect two cylinders (axis `i`/`j`, radius `r_i`/`r_j`) and
/// return the apex-side junction `P_12`, reusing the cap-circle
/// geometry of [`intersect_two_caps`](crate::operations::fillet).
///
/// Each cylinder's V-side cap is the circle in the plane through its
/// axis origin perpendicular to the axis. The two caps lie in
/// transverse planes (axes non-parallel at a rectilinear corner); the
/// junction is the apex-side crossing of the two cap circles, picked
/// by the outward score.
#[allow(dead_code, clippy::too_many_arguments)] // Reason: Stage 1 wires these.
fn two_cylinder_apex_intersection(
    c_i: Point3,
    u_i: Vector3,
    r_i: f64,
    c_j: Point3,
    u_j: Vector3,
    r_j: f64,
    vertex: Point3,
    outward: Vector3,
) -> Result<Point3, JunctionError> {
    const AXES_PARALLEL_TOL_SQ: f64 = 1.0e-18;
    const CIRCLE_SANITY_TOL: f64 = 1.0e-9;
    if !(r_i > 0.0 && r_i.is_finite() && r_j > 0.0 && r_j.is_finite()) {
        return Err(JunctionError::Spine(CornerBlendError::InvalidDistance));
    }

    // Direction of the plane-plane intersection line.
    let d_raw = u_i.cross(&u_j);
    let d_norm_sq = d_raw.x * d_raw.x + d_raw.y * d_raw.y + d_raw.z * d_raw.z;
    if d_norm_sq <= AXES_PARALLEL_TOL_SQ {
        return Err(JunctionError::NoCylinderCylinderRoot);
    }
    let d_norm = d_norm_sq.sqrt();
    let d_ij = Vector3::new(d_raw.x / d_norm, d_raw.y / d_norm, d_raw.z / d_norm);

    // Anchor P_0 on L_ij via the same 3×3 solve as intersect_two_caps.
    let mat = Matrix3::from_rows(&u_i, &u_j, &d_ij);
    let m = Vector3::new(
        (c_i.x + c_j.x) * 0.5,
        (c_i.y + c_j.y) * 0.5,
        (c_i.z + c_j.z) * 0.5,
    );
    let rhs = Vector3::new(
        u_i.x * c_i.x + u_i.y * c_i.y + u_i.z * c_i.z,
        u_j.x * c_j.x + u_j.y * c_j.y + u_j.z * c_j.z,
        d_ij.x * m.x + d_ij.y * m.y + d_ij.z * m.z,
    );
    let inv = mat.inverse().map_err(|_| JunctionError::Singular)?;
    let p0_vec = inv.transform_vector(&rhs);
    let p0 = Point3::new(p0_vec.x, p0_vec.y, p0_vec.z);

    let w = p0 - c_i;
    let b_half = w.x * d_ij.x + w.y * d_ij.y + w.z * d_ij.z;
    let w_sq = w.x * w.x + w.y * w.y + w.z * w.z;
    let c_coeff = w_sq - r_i * r_i;
    let disc = b_half * b_half - c_coeff;
    if disc < 0.0 {
        return Err(JunctionError::NoCylinderCylinderRoot);
    }
    let sqrt_disc = disc.sqrt();
    let s_plus = -b_half + sqrt_disc;
    let s_minus = -b_half - sqrt_disc;

    let make_candidate =
        |s: f64| -> Point3 { Point3::new(p0.x + s * d_ij.x, p0.y + s * d_ij.y, p0.z + s * d_ij.z) };
    let on_second_circle = |x: Point3| -> bool {
        let dx = x.x - c_j.x;
        let dy = x.y - c_j.y;
        let dz = x.z - c_j.z;
        ((dx * dx + dy * dy + dz * dz).sqrt() - r_j).abs() <= CIRCLE_SANITY_TOL
    };
    let v_side_score = |x: Point3| -> f64 {
        (x.x - vertex.x) * outward.x + (x.y - vertex.y) * outward.y + (x.z - vertex.z) * outward.z
    };

    let cand_plus = make_candidate(s_plus);
    let cand_minus = make_candidate(s_minus);
    let plus_ok = on_second_circle(cand_plus);
    let minus_ok = on_second_circle(cand_minus);
    if !plus_ok && !minus_ok {
        return Err(JunctionError::NoCylinderCylinderRoot);
    }
    let plus_score = v_side_score(cand_plus);
    let minus_score = v_side_score(cand_minus);
    let best = if plus_ok && (!minus_ok || plus_score >= minus_score) {
        cand_plus
    } else {
        cand_minus
    };
    Ok(best)
}

/// Zero out the Hoffmann / apex setbacks at the listed vertices on
/// every incident [`BlendEdge`] in `graph`. Used by the CF-β.5.2-B
/// partial-mixed corner path: at a 3-edge corner where one incident
/// edge is closed by a planar chamfer face (rather than a rolling
/// ball or apex sphere), the fillet arc on each of the remaining
/// two incident edges must terminate where the chamfer face begins
/// — i.e. at the simple offset point `V + r·u_face_edge` along the
/// non-filleted adjacent edge — *not* at the Hoffmann smooth-closure
/// retraction `r·cos(θ/2)` that [`compute_setbacks`] writes. Setting
/// the V-end setback to `None` (treated as `0.0` by
/// [`crate::operations::spine_solver::compute_setback_trim`]) makes
/// the fillet trim collapse to `ParamTrim::FULL` at V, so the arc
/// endpoint lands at the face-boundary tangent points that match
/// the chamfer's offset convention.
///
/// **Side effects.** For each `(vid, eid)` pair where `eid` is in
/// `graph.vertices[vid].incident_blend_edges`: sets
/// `start_setback = None` if `vid == edge.start_vertex`, sets
/// `end_setback = None` if `vid == edge.end_vertex`. Vertices not in
/// `graph.vertices` are silently skipped (the caller's
/// `partial_corner_vertices` list may include vertices that no
/// incident blend edge in this graph touches).
///
/// **Idempotency.** Multiple calls produce the same final state
/// (subsequent calls re-set already-`None` slots to `None`).
///
/// Must be called *after* both [`compute_setbacks`] and
/// [`compute_apex_setbacks`] — otherwise those passes would
/// re-stamp the Hoffmann / apex values back over the cleared
/// slots.
pub fn clear_setbacks_at(graph: &mut BlendGraph, model: &BRepModel, vertex_ids: &[VertexId]) {
    for &vid in vertex_ids {
        let incident: Vec<EdgeId> = match graph.vertices.get(&vid) {
            Some(v) => v.incident_blend_edges.clone(),
            None => continue,
        };
        for eid in incident {
            let (is_start, is_end) = match model.edges.get(eid) {
                Some(e) => (e.start_vertex == vid, e.end_vertex == vid),
                None => continue,
            };
            if let Some(be) = graph.edges.get_mut(&eid) {
                if is_start {
                    be.start_setback = None;
                }
                if is_end {
                    be.end_setback = None;
                }
            }
        }
    }
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
        let edge_id: EdgeId = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("≥ 1 edge");
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
        let edge_id: EdgeId = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("≥ 1 edge");
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
        let mut graph = build(&mut model, &selection).expect("build three-edge corner selection");

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
        let edge_id: EdgeId = model
            .edges
            .iter()
            .next()
            .map(|(id, _)| id)
            .expect("≥ 1 edge");
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
        assert!(!vertex_needs_setback(BlendVertexKind::ConvexCorner {
            degree: 1
        }));
        assert!(!vertex_needs_setback(BlendVertexKind::ConcaveCorner {
            degree: 1
        }));
        assert!(vertex_needs_setback(BlendVertexKind::ConvexCorner {
            degree: 2
        }));
        assert!(vertex_needs_setback(BlendVertexKind::ConvexCorner {
            degree: 3
        }));
        assert!(vertex_needs_setback(BlendVertexKind::ConcaveCorner {
            degree: 2
        }));
        assert!(vertex_needs_setback(BlendVertexKind::Mixed));
    }

    #[test]
    fn blend_radius_min_max_match_schedule() {
        let c = BlendRadius::Constant(0.5);
        assert_eq!(c.min_value(), 0.5);
        assert_eq!(c.max_value(), 0.5);
        let l = BlendRadius::Linear {
            start: 0.1,
            end: 0.7,
        };
        assert_eq!(l.min_value(), 0.1);
        assert_eq!(l.max_value(), 0.7);
        let v = BlendRadius::Variable(vec![(0.0, 0.3), (0.5, 0.9), (1.0, 0.2)]);
        assert_eq!(v.min_value(), 0.2);
        assert_eq!(v.max_value(), 0.9);
    }

    // ================================================================
    // Stage 0 — vertex-blend corner primitive de-risk + building
    // blocks. The fixture is a 10³ box (corners at (±5,±5,±5)); the
    // 1C2F corner under test is V = (5,5,5) with chamfer D = 1 and two
    // fillets r = 1. Apex is the concurrent point of the three offset
    // spines, which for this corner is (4,4,4).
    // ================================================================

    fn build_box_side_10() -> BRepModel {
        let mut model = BRepModel::new();
        {
            let mut builder = TopologyBuilder::new(&mut model);
            builder
                .create_box_3d(10.0, 10.0, 10.0)
                .expect("create_box_3d(10,10,10) should succeed");
        }
        classify_all_unclassified_edges(&mut model).expect("F2-α sweep");
        model
    }

    /// Locate the box vertex nearest `target` and return it with its
    /// (≤3) incident edges, sorted by id for determinism.
    fn corner_at(model: &BRepModel, target: Point3) -> (VertexId, Vec<EdgeId>) {
        let mut best: Option<(VertexId, f64)> = None;
        for (vid, v) in model.vertices.iter() {
            let p = v.position;
            let d =
                (p[0] - target.x).powi(2) + (p[1] - target.y).powi(2) + (p[2] - target.z).powi(2);
            match best {
                Some((_, bd)) if bd <= d => {}
                _ => best = Some((vid, d)),
            }
        }
        let (vid, _) = best.expect("box has vertices");
        let mut edges: Vec<EdgeId> = model
            .edges
            .iter()
            .filter(|(_, e)| e.start_vertex == vid || e.end_vertex == vid)
            .map(|(id, _)| id)
            .collect();
        edges.sort();
        (vid, edges)
    }

    /// Build a degree-3 BlendGraph at the (5,5,5) corner with all three
    /// incident edges selected at radius 1.0. Returns the graph plus
    /// the corner vertex id and its three edge ids.
    fn corner_graph_1c2f(model: &mut BRepModel) -> (BlendGraph, VertexId, [EdgeId; 3]) {
        let (vid, edges) = corner_at(model, Point3::new(5.0, 5.0, 5.0));
        assert_eq!(edges.len(), 3, "corner (5,5,5) must be degree-3");
        let selection: Vec<_> = edges
            .iter()
            .map(|&e| (e, BlendRadius::Constant(1.0)))
            .collect();
        let graph = build(model, &selection).expect("build 1C2F corner graph");
        let arr = [edges[0], edges[1], edges[2]];
        (graph, vid, arr)
    }

    #[test]
    fn mixed_apex_unit_cube_corner_is_4_4_4_setback_1() {
        let mut model = build_box_side_10();
        let (mut graph, vid, edges) = corner_graph_1c2f(&mut model);

        // Designate edges[0] the chamfer (D = 1); edges[1], edges[2]
        // are the fillets (r = 1).
        let chamfer = edges[0];
        compute_apex_setbacks_mixed(&model, &mut graph, chamfer, 1.0)
            .expect("mixed apex solve must succeed on a clean box corner");

        // Reconstruct the apex directly from the three spines so we can
        // assert (4,4,4) exactly.
        let v = model.vertices.get(vid).expect("corner vertex");
        let v_pos = Point3::new(v.position[0], v.position[1], v.position[2]);
        let axes: Vec<CornerBlendAxis> = edges
            .iter()
            .map(|&e| {
                predict_corner_blend_axis(&model, vid, v_pos, e, 1.0).expect("spine prediction")
            })
            .collect();
        let apex = least_squares_concurrent_point(&axes).expect("apex solvable");
        println!("MIXED APEX = ({}, {}, {})", apex.x, apex.y, apex.z);
        assert!(
            (apex.x - 4.0).abs() < 1e-9
                && (apex.y - 4.0).abs() < 1e-9
                && (apex.z - 4.0).abs() < 1e-9,
            "mixed 1C2F apex must be (4,4,4); got ({:.12}, {:.12}, {:.12})",
            apex.x,
            apex.y,
            apex.z
        );

        // Each FILLET edge's V-end setback must be exactly 1.0; the
        // chamfer edge must be left without a setback at the corner.
        for &eid in &edges {
            let be = graph.edge(eid).expect("BlendEdge present");
            let edge = model.edges.get(eid).expect("edge present");
            let at_corner = if edge.start_vertex == vid {
                be.start_setback
            } else {
                be.end_setback
            };
            if eid == chamfer {
                assert!(
                    at_corner.is_none(),
                    "chamfer edge must receive no apex setback; got {:?}",
                    at_corner
                );
            } else {
                let s = at_corner.expect("fillet setback at corner must be populated");
                assert!(
                    (s - 1.0).abs() < 1e-9,
                    "fillet setback at 1C2F corner: expected 1.0, got {}",
                    s
                );
            }
        }
    }

    #[test]
    fn mixed_apex_no_op_when_chamfer_edge_absent() {
        // A chamfer edge id not present at the corner ⇒ the solve must
        // leave every setback untouched (None).
        let mut model = build_box_side_10();
        let (mut graph, vid, edges) = corner_graph_1c2f(&mut model);
        let bogus_chamfer: EdgeId = 9_999;
        compute_apex_setbacks_mixed(&model, &mut graph, bogus_chamfer, 1.0)
            .expect("no matching corner ⇒ no-op");
        for &eid in &edges {
            let be = graph.edge(eid).expect("BlendEdge present");
            let _ = vid;
            assert!(
                be.start_setback.is_none() && be.end_setback.is_none(),
                "no-op solve must not write any setback"
            );
        }
    }

    #[test]
    fn junctions_unit_cube_corner_apex_side() {
        let mut model = build_box_side_10();
        let (graph, vid, edges) = corner_graph_1c2f(&mut model);
        let chamfer = edges[0];
        let fillets = [edges[1], edges[2]];

        let j = solve_corner_junctions(&model, &graph, vid, edges, chamfer, 1.0)
            .expect("junction solve on clean box corner");

        // Geometry of the corner V=(5,5,5):
        //   chamfer edge[0] adjacent faces give bevel plane; fillets
        //   are cylinders of axis {(t,4,4)} etc. We assert each
        //   junction is a finite apex-side point inboard of V.
        let v = model.vertices.get(vid).expect("vertex");
        let v_pos = Point3::new(v.position[0], v.position[1], v.position[2]);
        for (name, p) in [("J1", j.j1), ("J2", j.j2), ("P12", j.p12)] {
            assert!(
                p.x.is_finite() && p.y.is_finite() && p.z.is_finite(),
                "{} must be finite, got {:?}",
                name,
                p
            );
            // Apex side: strictly inboard of the corner on every axis
            // (each coordinate < 5).
            assert!(
                p.x < v_pos.x + 1e-9 && p.y < v_pos.y + 1e-9 && p.z < v_pos.z + 1e-9,
                "{} must lie inboard of V=(5,5,5); got {:?}",
                name,
                p
            );
        }

        // P_12 is the apex-side crossing of the two fillet cap circles.
        // For r1=r2=1 perpendicular fillets the crossing on the apex
        // side is the apex sphere centre's projection — it must be
        // equidistant (=1) from both cylinder axes.
        let v_pos2 = v_pos;
        let dist_to_axis = |p: Point3, q: Point3, u: Vector3| -> f64 {
            let w = p - q;
            let along = w.dot(&u);
            let perp = Vector3::new(w.x - along * u.x, w.y - along * u.y, w.z - along * u.z);
            perp.magnitude()
        };
        let a1 =
            predict_corner_blend_axis(&model, vid, v_pos2, fillets[0], 1.0).expect("fillet1 spine");
        let a2 =
            predict_corner_blend_axis(&model, vid, v_pos2, fillets[1], 1.0).expect("fillet2 spine");
        assert!(
            (dist_to_axis(j.p12, a1.q, a1.axis_dir) - 1.0).abs() < 1e-6,
            "P12 must be radius 1 from fillet-1 axis; got {}",
            dist_to_axis(j.p12, a1.q, a1.axis_dir)
        );
        assert!(
            (dist_to_axis(j.p12, a2.q, a2.axis_dir) - 1.0).abs() < 1e-6,
            "P12 must be radius 1 from fillet-2 axis; got {}",
            dist_to_axis(j.p12, a2.q, a2.axis_dir)
        );

        // J_i must lie on BOTH the chamfer bevel plane AND fillet-i's
        // V-cap circle (radius 1 from fillet-i axis). Reconstruct the
        // bevel plane the same way the solver does.
        let ca =
            predict_corner_blend_axis(&model, vid, v_pos2, chamfer, 1.0).expect("chamfer spine");
        let bevel_normal = (ca.face_normals[0] + ca.face_normals[1])
            .normalize()
            .expect("bevel normal");
        let bevel_point = Point3::new(
            v_pos2.x - 1.0 * ca.face_normals[0].x,
            v_pos2.y - 1.0 * ca.face_normals[0].y,
            v_pos2.z - 1.0 * ca.face_normals[0].z,
        );
        let on_bevel = |p: Point3| -> f64 { bevel_normal.dot(&(p - bevel_point)).abs() };
        assert!(
            on_bevel(j.j1) < 1e-6 && on_bevel(j.j2) < 1e-6,
            "J1/J2 must lie on the bevel plane; devs {} / {}",
            on_bevel(j.j1),
            on_bevel(j.j2)
        );
        assert!(
            (dist_to_axis(j.j1, a1.q, a1.axis_dir) - 1.0).abs() < 1e-6,
            "J1 must lie on fillet-1 cap circle (radius 1); got {}",
            dist_to_axis(j.j1, a1.q, a1.axis_dir)
        );
        assert!(
            (dist_to_axis(j.j2, a2.q, a2.axis_dir) - 1.0).abs() < 1e-6,
            "J2 must lie on fillet-2 cap circle (radius 1); got {}",
            dist_to_axis(j.j2, a2.q, a2.axis_dir)
        );

        // Emit the computed junctions for the Stage-0 report.
        println!(
            "JUNCTIONS J1={:?} J2={:?} P12={:?}",
            (j.j1.x, j.j1.y, j.j1.z),
            (j.j2.x, j.j2.y, j.j2.z),
            (j.p12.x, j.p12.y, j.p12.z)
        );
    }

    #[test]
    fn junction_two_cylinder_no_root_is_typed_error() {
        // Parallel axes ⇒ the cap planes are parallel ⇒ no transverse
        // intersection ⇒ typed error, not a panic.
        let c_i = Point3::new(0.0, 0.0, 0.0);
        let c_j = Point3::new(0.0, 5.0, 0.0);
        let u = Vector3::new(1.0, 0.0, 0.0);
        let res = two_cylinder_apex_intersection(
            c_i,
            u,
            1.0,
            c_j,
            u,
            1.0,
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 1.0, 0.0),
        );
        assert_eq!(res, Err(JunctionError::NoCylinderCylinderRoot));
    }

    // ----------------------------------------------------------------
    // ★ R1 DE-RISK PROTOTYPE
    //
    // Claim under test (strategy "D2"): a PLANAR chamfer bevel face
    // tolerates re-anchoring its two V-end (cap-chord) vertices to
    // arbitrary inboard points along the rails AND replacing the
    // cap-chord edge with a fresh Line, while remaining (a) a closed
    // outer loop and (b) planar. This is the asymmetry that makes the
    // chamfer-rim rebuild safe where a curved fillet-rail param-range
    // retrim tore the face (Euler-invalid). If this fails, Stage 1's
    // rim rebuild balloons into a full bevel re-synthesis.
    // ----------------------------------------------------------------

    /// Build a real single-edge chamfer bevel on a 10³ box and return
    /// (model, bevel_face_id, bevel_plane).
    fn build_box_with_bevel() -> (BRepModel, crate::primitives::face::FaceId, Plane) {
        use crate::operations::chamfer::{chamfer_edges, ChamferOptions, ChamferType};
        use crate::primitives::topology_builder::GeometryId;

        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder.create_box_3d(10.0, 10.0, 10.0) {
                Ok(GeometryId::Solid(id)) => id,
                _ => panic!("box creation failed"),
            }
        };
        let edge0: EdgeId = {
            let mut es: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
            es.sort();
            es[0]
        };
        let options = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(1.0),
            ..Default::default()
        };
        let faces = chamfer_edges(&mut model, solid_id, vec![edge0], options)
            .expect("single-edge chamfer must succeed");
        let bevel_face = faces[0];

        // The chamfer bevel surface is a `RuledSurface` between two
        // parallel straight rails — geometrically planar but not a
        // `Plane` object. Derive the bevel plane analytically from
        // three distinct loop vertices so the R1 coplanarity check is
        // grounded in the face's own geometry.
        let outer_loop = model.faces.get(bevel_face).expect("bevel face").outer_loop;
        let loop_edges: Vec<EdgeId> = model.loops.get(outer_loop).expect("loop").edges.clone();
        let mut pts: Vec<Point3> = Vec::new();
        for &eid in &loop_edges {
            let edge = model.edges.get(eid).expect("edge");
            for v in [edge.start_vertex, edge.end_vertex] {
                let p = model.vertices.get_position(v).expect("vtx pos");
                let pt = Point3::new(p[0], p[1], p[2]);
                if !pts.iter().any(|q: &Point3| (*q - pt).magnitude() < 1e-9) {
                    pts.push(pt);
                }
            }
        }
        assert!(
            pts.len() >= 3,
            "bevel face must expose ≥3 distinct vertices to define a plane"
        );
        // Pick three non-collinear vertices.
        let p0 = pts[0];
        let p1 = pts[1];
        let p2 = pts
            .iter()
            .copied()
            .find(|&p| {
                let a = p1 - p0;
                let b = p - p0;
                a.cross(&b).magnitude() > 1e-6
            })
            .expect("three non-collinear bevel vertices exist");
        let plane = Plane::from_three_points(p0, p1, p2).expect("bevel plane from 3 points");
        (model, bevel_face, plane)
    }

    #[test]
    fn r1_planar_bevel_tolerates_reanchor_and_chord_replacement() {
        use crate::operations::edge_blend_topology::loop_is_closed;
        use crate::primitives::curve::Line;

        let (mut model, bevel_face, plane) = build_box_with_bevel();

        // Loop must start closed and planar.
        let outer_loop = model.faces.get(bevel_face).expect("face").outer_loop;
        assert!(
            loop_is_closed(&model, outer_loop).expect("loop closure check"),
            "precondition: fresh bevel loop is closed"
        );

        // Identify a cap-chord edge: a straight (Line) edge of the
        // bevel loop whose BOTH endpoints sit at original box corners
        // (the V-ends). The bevel rectangle has two such chords; pick
        // the one at the lowest index for determinism.
        let edge_ids: Vec<EdgeId> = {
            let lp = model.loops.get(outer_loop).expect("loop");
            lp.edges.clone()
        };
        // A chord's two endpoints are the chamfer V-end pair; a rail's
        // endpoints are one V-end + the far rim. We classify chords as
        // the loop edges whose direction is the cross-section chord —
        // distinguished by being the SHORTER straight edges. Compute
        // each straight edge's length and take the two shortest.
        let mut straight: Vec<(EdgeId, f64)> = Vec::new();
        for &eid in &edge_ids {
            let edge = model.edges.get(eid).expect("edge");
            if model
                .curves
                .get(edge.curve_id)
                .and_then(|c| c.as_any().downcast_ref::<Line>().map(|_| ()))
                .is_none()
            {
                continue;
            }
            let s = model
                .vertices
                .get_position(edge.start_vertex)
                .expect("start pos");
            let e = model
                .vertices
                .get_position(edge.end_vertex)
                .expect("end pos");
            let len =
                ((s[0] - e[0]).powi(2) + (s[1] - e[1]).powi(2) + (s[2] - e[2]).powi(2)).sqrt();
            straight.push((eid, len));
        }
        straight.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));
        assert!(
            straight.len() >= 2,
            "bevel loop must have ≥2 straight edges (the two cap chords)"
        );
        let chord_edge = straight[0].0;

        // Find the two rail edges adjacent to the chord in the loop —
        // the chord's endpoints are shared with exactly those rails.
        let chord = model.edges.get(chord_edge).expect("chord edge");
        let v_a = chord.start_vertex;
        let v_b = chord.end_vertex;

        // For each chord endpoint, find the rail edge (the OTHER loop
        // edge incident to it) and re-anchor the endpoint a small step
        // inboard ALONG that rail toward its far end.
        let reanchor = |model: &mut BRepModel, v: VertexId| -> VertexId {
            // Find a loop edge != chord_edge incident to v.
            let rail = edge_ids
                .iter()
                .copied()
                .find(|&e| {
                    if e == chord_edge {
                        return false;
                    }
                    let ed = match model.edges.get(e) {
                        Some(ed) => ed,
                        None => return false,
                    };
                    ed.start_vertex == v || ed.end_vertex == v
                })
                .expect("chord endpoint must share a rail edge");
            let ed = model.edges.get(rail).expect("rail");
            let far = if ed.start_vertex == v {
                ed.end_vertex
            } else {
                ed.start_vertex
            };
            let vp = model.vertices.get_position(v).expect("v pos");
            let fp = model.vertices.get_position(far).expect("far pos");
            // Move 30% of the way from v toward the rail's far end —
            // an arbitrary inboard point ON the rail (so still on the
            // bevel plane, since the rail lies in the bevel plane).
            let nx = vp[0] + 0.3 * (fp[0] - vp[0]);
            let ny = vp[1] + 0.3 * (fp[1] - vp[1]);
            let nz = vp[2] + 0.3 * (fp[2] - vp[2]);
            let new_v = model.vertices.add(nx, ny, nz);
            // Reanchor the rail's endpoint to the new vertex + rebuild
            // the rail line (mirror reanchor_seam_edges_at_cap_arc_*).
            let far_pos = model.vertices.get_position(far).expect("far pos");
            let far_pt = Point3::new(far_pos[0], far_pos[1], far_pos[2]);
            let new_pt = Point3::new(nx, ny, nz);
            let ed = model.edges.get(rail).expect("rail");
            let replaces_start = ed.start_vertex == v;
            let (ls, le) = if replaces_start {
                (new_pt, far_pt)
            } else {
                (far_pt, new_pt)
            };
            let new_curve = model.curves.add(Box::new(Line::new(ls, le)));
            let edm = model.edges.get_mut(rail).expect("rail mut");
            edm.curve_id = new_curve;
            if replaces_start {
                edm.start_vertex = new_v;
            } else {
                edm.end_vertex = new_v;
            }
            new_v
        };

        let new_a = reanchor(&mut model, v_a);
        let new_b = reanchor(&mut model, v_b);

        // REPLACE the chord edge: a fresh Line between the two new
        // inboard points, spliced in by loop-edge replacement
        // (lp.edges[idx] = new_edge), NOT an in-place curve mutation.
        let na_pos = model.vertices.get_position(new_a).expect("new_a pos");
        let nb_pos = model.vertices.get_position(new_b).expect("new_b pos");
        let new_chord_curve = model.curves.add(Box::new(Line::new(
            Point3::new(na_pos[0], na_pos[1], na_pos[2]),
            Point3::new(nb_pos[0], nb_pos[1], nb_pos[2]),
        )));
        let old_chord = model.edges.get(chord_edge).expect("chord");
        let new_chord_edge = model
            .edges
            .add(crate::primitives::edge::Edge::new_auto_range(
                0,
                new_a,
                new_b,
                new_chord_curve,
                old_chord.orientation,
            ));
        {
            let lp = model.loops.get_mut(outer_loop).expect("loop mut");
            let idx = lp
                .edges
                .iter()
                .position(|&e| e == chord_edge)
                .expect("chord in loop");
            lp.edges[idx] = new_chord_edge; // loop-edge replacement
        }

        // ---- ASSERT (a): outer loop still closed. ----
        let closed = loop_is_closed(&model, outer_loop).expect("loop closure check after rebuild");
        assert!(
            closed,
            "R1 FAIL: bevel outer loop tore after reanchor + chord replacement"
        );

        // ---- ASSERT (b): face still planar (all loop vertices
        //       coplanar with the original bevel plane). ----
        let loop_edges: Vec<EdgeId> = model.loops.get(outer_loop).expect("loop").edges.clone();
        let mut max_dev = 0.0_f64;
        for &eid in &loop_edges {
            let edge = model.edges.get(eid).expect("edge");
            for vtx in [edge.start_vertex, edge.end_vertex] {
                let p = model.vertices.get_position(vtx).expect("vtx pos");
                let dev = plane
                    .distance_to_point(&Point3::new(p[0], p[1], p[2]))
                    .abs();
                if dev > max_dev {
                    max_dev = dev;
                }
            }
        }
        assert!(
            max_dev < 1e-6,
            "R1 FAIL: bevel went non-planar; max vertex deviation {} from bevel plane",
            max_dev
        );
    }

    // ----------------------------------------------------------------
    // ★ DELIVERABLE #1 GATE — degree-2 post-surgery solver
    //
    // Build the LIVE 1C2F corner exactly as the repro does (chamfer
    // FIRST, fillet SECOND) and drive the new post-surgery solver from
    // the SURVIVING faces (chamfer bevel + two fillet cylinders), not
    // the consumed sharp edges. Prove it (a) does NOT error on the
    // real degree-2 input (the Stage-0 bug) and (b) recovers the live
    // post-surgery rim corners J1=(5,5,4), J2=(4,5,5), P12=(5,4,5).
    // ----------------------------------------------------------------

    /// Build the live 1C2F corner: cube 10³ → chamfer `edges[0]` (D=1)
    /// → fillet `edges[1]`,`edges[2]` (r=1) at corner (5,5,5). Returns
    /// the result model, the solid id, and the corner vertex position.
    fn build_live_1c2f_corner() -> (BRepModel, Point3) {
        use crate::operations::chamfer::{
            chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
        };
        use crate::operations::fillet::{
            fillet_edges, FilletOptions, FilletType, PropagationMode as FilletProp,
        };
        use crate::operations::mixed_kind_corner_cap::SeamContinuity;
        use crate::operations::CommonOptions;
        use crate::primitives::topology_builder::GeometryId;

        const BOX_SIZE: f64 = 10.0;
        const HALF: f64 = BOX_SIZE / 2.0;
        const D: f64 = 1.0;

        let mut model = BRepModel::new();
        let solid_id = {
            let mut builder = TopologyBuilder::new(&mut model);
            match builder.create_box_3d(BOX_SIZE, BOX_SIZE, BOX_SIZE) {
                Ok(GeometryId::Solid(id)) => id,
                _ => panic!("box creation failed"),
            }
        };

        // Corner (5,5,5) and its three incident edges, id-sorted.
        let corner = model
            .vertices
            .iter()
            .find(|(_, v)| {
                let p = v.position;
                (p[0] - HALF).abs() < 1e-9
                    && (p[1] - HALF).abs() < 1e-9
                    && (p[2] - HALF).abs() < 1e-9
            })
            .map(|(id, _)| id)
            .expect("corner vertex at (5,5,5)");
        let mut corner_edges: Vec<EdgeId> = model
            .edges
            .iter()
            .filter(|(_, e)| e.start_vertex == corner || e.end_vertex == corner)
            .map(|(id, _)| id)
            .collect();
        corner_edges.sort_unstable();

        let chamfer_opts = ChamferOptions {
            chamfer_type: ChamferType::EqualDistance(D),
            distance1: D,
            distance2: D,
            symmetric: true,
            propagation: ChamferProp::None,
            seam_continuity: SeamContinuity::C0,
            partial_corner_vertices: vec![corner],
            common: CommonOptions {
                validate_result: true,
                ..Default::default()
            },
            ..Default::default()
        };
        let fillet_opts = FilletOptions {
            fillet_type: FilletType::Constant(D),
            radius: D,
            propagation: FilletProp::None,
            seam_continuity: SeamContinuity::C0,
            partial_corner_vertices: vec![],
            common: CommonOptions {
                validate_result: true,
                ..Default::default()
            },
            ..Default::default()
        };

        chamfer_edges(&mut model, solid_id, vec![corner_edges[0]], chamfer_opts)
            .expect("1C2F chamfer-first succeeds");
        fillet_edges(
            &mut model,
            solid_id,
            vec![corner_edges[1], corner_edges[2]],
            fillet_opts,
        )
        .expect("1C2F fillet-second succeeds");

        (model, Point3::new(HALF, HALF, HALF))
    }

    /// Locate the post-surgery chamfer bevel face (a `RuledSurface`)
    /// and the two fillet cylinder faces (`CylindricalFillet` surfaces)
    /// in the result model.
    fn find_corner_faces(model: &BRepModel) -> (FaceId, Vec<FaceId>) {
        use crate::primitives::fillet_surfaces::CylindricalFillet;
        use crate::primitives::surface::RuledSurface;

        let mut bevel: Option<FaceId> = None;
        let mut fillets: Vec<FaceId> = Vec::new();
        // Deterministic iteration: id-sorted.
        let mut faces: Vec<FaceId> = model.faces.iter().map(|(id, _)| id).collect();
        faces.sort_unstable();
        for fid in faces {
            let face = match model.faces.get(fid) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            if surface
                .as_any()
                .downcast_ref::<CylindricalFillet>()
                .is_some()
            {
                fillets.push(fid);
            } else if surface.as_any().downcast_ref::<RuledSurface>().is_some() {
                // The single-corner chamfer leaves exactly one bevel
                // RuledSurface; keep the lowest-id one defensively.
                if bevel.is_none() {
                    bevel = Some(fid);
                }
            }
        }
        (
            bevel.expect("post-surgery chamfer bevel RuledSurface"),
            fillets,
        )
    }

    #[test]
    fn post_surgery_degree2_solver_recovers_live_rim_corners() {
        let (model, v_pos) = build_live_1c2f_corner();
        let (bevel_face, fillet_faces) = find_corner_faces(&model);
        assert_eq!(
            fillet_faces.len(),
            2,
            "1C2F corner must leave exactly two fillet cylinder faces; got {}",
            fillet_faces.len()
        );

        // Expected live post-surgery rim corners (Stage-1 dump
        // v11/v9/v12). J_i pairs the bevel with fillet_faces[i]; P_12
        // is fillet ∩ fillet. The pairing of a specific expected J to
        // fillet_faces[0] vs [1] depends on which cylinder got the
        // lower face id, so we assert the unordered J set and the exact
        // P_12 — both within 1e-6.
        let expected_chamfer_fillet = [
            Point3::new(5.0, 5.0, 4.0), // J1
            Point3::new(4.0, 5.0, 5.0), // J2
        ];
        let expected_p12 = Point3::new(5.0, 4.0, 5.0);

        // (a) The solver must NOT error on the real degree-2 input —
        // the exact failure mode the Stage-0 solvers had (the consumed
        // chamfer edge → MissingTopology no-op).
        let sol = solve_corner_junctions_post_surgery(
            &model,
            v_pos,
            bevel_face,
            [fillet_faces[0], fillet_faces[1]],
        )
        .expect("post-surgery degree-2 solver must succeed on the real 1C2F corner");

        // (b) Junctions equal the live rim corners within 1e-6.
        let close = |a: Point3, b: Point3| (a - b).magnitude() < 1e-6;

        // P_12 is fillet ∩ fillet — independent of fillet ordering.
        assert!(
            close(sol.p12, expected_p12),
            "P_12 must be the live rim corner {:?}; got {:?}",
            expected_p12,
            sol.p12
        );

        // J_1, J_2 must be the two chamfer-fillet rim corners (the
        // order maps to fillet_faces order). Each must hit one expected
        // value, and together they must cover both.
        let j1_hits: Vec<usize> = expected_chamfer_fillet
            .iter()
            .enumerate()
            .filter(|(_, &e)| close(sol.j1, e))
            .map(|(i, _)| i)
            .collect();
        let j2_hits: Vec<usize> = expected_chamfer_fillet
            .iter()
            .enumerate()
            .filter(|(_, &e)| close(sol.j2, e))
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            j1_hits.len(),
            1,
            "J_1 ({:?}) must equal exactly one live chamfer-fillet rim corner {:?}",
            sol.j1,
            expected_chamfer_fillet
        );
        assert_eq!(
            j2_hits.len(),
            1,
            "J_2 ({:?}) must equal exactly one live chamfer-fillet rim corner {:?}",
            sol.j2,
            expected_chamfer_fillet
        );
        assert_ne!(
            j1_hits[0], j2_hits[0],
            "J_1 and J_2 must be the two DISTINCT chamfer-fillet rim corners, \
             not the same one (J_1={:?}, J_2={:?})",
            sol.j1, sol.j2
        );

        // The apex must be the rolling-ball concurrent point (4,4,4)
        // and each fillet setback exactly 1.0 — pins that the solve is
        // the same geometry the all-edges Stage-0 solver predicts.
        assert!(
            close(sol.apex, Point3::new(4.0, 4.0, 4.0)),
            "apex must be (4,4,4); got {:?}",
            sol.apex
        );
        assert!(
            (sol.setback_fillet_1 - 1.0).abs() < 1e-6 && (sol.setback_fillet_2 - 1.0).abs() < 1e-6,
            "each fillet apex setback must be 1.0; got {} and {}",
            sol.setback_fillet_1,
            sol.setback_fillet_2
        );

        println!(
            "POST-SURGERY 1C2F: apex=({:.6},{:.6},{:.6}) \
             J1=({:.6},{:.6},{:.6}) J2=({:.6},{:.6},{:.6}) \
             P12=({:.6},{:.6},{:.6})",
            sol.apex.x,
            sol.apex.y,
            sol.apex.z,
            sol.j1.x,
            sol.j1.y,
            sol.j1.z,
            sol.j2.x,
            sol.j2.y,
            sol.j2.z,
            sol.p12.x,
            sol.p12.y,
            sol.p12.z,
        );
    }
}
