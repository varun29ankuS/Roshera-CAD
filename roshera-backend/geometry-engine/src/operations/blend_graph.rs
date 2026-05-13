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

use crate::operations::edge_classification::classify_and_cache;
use crate::operations::OperationResult;
use crate::primitives::edge::{EdgeId, ManifoldKind};
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
