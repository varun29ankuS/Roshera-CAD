//! Feature bounding-volume hierarchy + broad-phase culling (CD-φ.6.1).
//!
//! The leaves are the **supermaximal features** ([`crate::queries::features`]),
//! not raw B-Rep faces — so the hierarchy reasons about CD entities directly.
//! Each node carries two co-located bounding objects (Crozet, *Smooth-BRep CD*,
//! Sec 2.4):
//!
//! * an **AABB** over the feature geometry — the spatial bound, and
//! * a **polyhedral normal cone** (the union of the subtree's feature cones) —
//!   the directional bound.
//!
//! A node-pair is culled in the **broad phase** ([`FeatureBvh::candidate_pairs`])
//! when *either* the AABBs are disjoint (can't be near) *or* the normal cones
//! can't oppose (can't form a contact — the same [`features_can_contact`] test
//! the cone substrate already provides). What survives is the small set of
//! feature-pairs handed to the LMD engine. The traversal counts its node visits,
//! so the broad-phase cost is a measured number, feeding the ablation harness.
//!
//! AABBs are built from the faces' boundary vertices plus a surface sample over
//! their parameter extent (so a curved face's bulge is captured, not just its
//! corners). Tighter OBBs (Sec 2.4.2) and a compatibility-mask cull are
//! refinements layered on this structure later.

use crate::math::bbox::BBox;
use crate::math::polyhedral_cone::{ConeIntersectionResult, PolyhedralCone};
use crate::math::vector3::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::queries::cd::features_can_contact;
use crate::queries::features::{feature_normal_cone, supermaximal_features, SupermaximalFeature};
use std::collections::HashMap;

/// What a BVH node points to.
#[derive(Debug, Clone, Copy)]
enum NodePayload {
    /// Index into [`FeatureBvh::features`].
    Leaf(usize),
    /// Left and right child node indices.
    Internal(usize, usize),
}

#[derive(Debug, Clone)]
struct BvhNode {
    aabb: BBox,
    cone: PolyhedralCone,
    payload: NodePayload,
}

/// A bounding-volume hierarchy over a solid's supermaximal features.
#[derive(Debug, Clone)]
pub struct FeatureBvh {
    features: Vec<SupermaximalFeature>,
    nodes: Vec<BvhNode>,
    root: Option<usize>,
}

/// Result of a broad-phase query between two BVHs.
#[derive(Debug, Clone)]
pub struct BroadPhaseResult {
    /// Surviving `(feature index in self, feature index in other)` pairs — the
    /// candidates handed to the narrow-phase LMD engine.
    pub pairs: Vec<(usize, usize)>,
    /// Node-pair tests performed during traversal — the broad-phase cost metric
    /// (compare against the brute `features(a) × features(b)` pair count).
    pub node_visits: usize,
}

impl FeatureBvh {
    /// Build the BVH over a solid's supermaximal features.
    pub fn build(model: &BRepModel, solid_id: SolidId) -> FeatureBvh {
        let features = supermaximal_features(model, solid_id);
        let leaf_data: Vec<(BBox, PolyhedralCone)> = features
            .iter()
            .map(|f| (feature_aabb(model, f), feature_normal_cone(model, f)))
            .collect();

        let mut nodes = Vec::new();
        let root = if features.is_empty() {
            None
        } else {
            let mut order: Vec<usize> = (0..features.len()).collect();
            Some(build_subtree(&mut order, &leaf_data, &mut nodes))
        };
        FeatureBvh {
            features,
            nodes,
            root,
        }
    }

    /// The supermaximal features this BVH indexes (leaf order).
    pub fn features(&self) -> &[SupermaximalFeature] {
        &self.features
    }

    /// Number of leaf features.
    pub fn feature_count(&self) -> usize {
        self.features.len()
    }

    /// Total node count (a full binary tree over `n` leaves has `2n − 1`).
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// AABB of the whole solid (root node), or `None` if it has no features.
    pub fn root_aabb(&self) -> Option<BBox> {
        self.root.map(|r| self.nodes[r].aabb)
    }

    /// **Broad phase.** Surviving feature-pairs after AABB-overlap and normal-cone
    /// culling, traversing both trees and pruning whole subtrees.
    pub fn candidate_pairs(&self, other: &FeatureBvh) -> BroadPhaseResult {
        let mut pairs = Vec::new();
        let mut node_visits = 0usize;
        if let (Some(ra), Some(rb)) = (self.root, other.root) {
            let mut stack = vec![(ra, rb)];
            while let Some((ai, bi)) = stack.pop() {
                node_visits += 1;
                let a = &self.nodes[ai];
                let b = &other.nodes[bi];
                if !a.aabb.intersects(&b.aabb) {
                    continue; // spatially apart
                }
                if !features_can_contact(&a.cone, &b.cone) {
                    continue; // directionally incapable of contact
                }
                match (a.payload, b.payload) {
                    (NodePayload::Leaf(fa), NodePayload::Leaf(fb)) => pairs.push((fa, fb)),
                    (NodePayload::Internal(l, r), _) => {
                        stack.push((l, bi));
                        stack.push((r, bi));
                    }
                    (NodePayload::Leaf(_), NodePayload::Internal(l, r)) => {
                        stack.push((ai, l));
                        stack.push((ai, r));
                    }
                }
            }
        }
        BroadPhaseResult { pairs, node_visits }
    }

    /// Structural invariants, for tests: every internal node's AABB contains
    /// both children's AABBs, and its normal cone contains both children's
    /// generators (the bounds are genuinely conservative bottom-up).
    pub fn well_formed(&self) -> bool {
        for node in &self.nodes {
            if let NodePayload::Internal(l, r) = node.payload {
                let lc = &self.nodes[l];
                let rc = &self.nodes[r];
                if !node.aabb.contains_bbox(&lc.aabb) || !node.aabb.contains_bbox(&rc.aabb) {
                    return false;
                }
                let child_gens = lc.cone.generators().iter().chain(rc.cone.generators());
                for g in child_gens {
                    if !node.cone.contains(g) {
                        return false;
                    }
                }
            }
        }
        true
    }
}

// ---------------------------------------------------------------------------
// build (private)
// ---------------------------------------------------------------------------

/// Recursively build a subtree over the leaf indices in `order`, pushing nodes
/// into `nodes` and returning the subtree root's node index. Splits at the
/// spatial median along the axis of greatest centroid spread.
fn build_subtree(
    order: &mut [usize],
    leaf_data: &[(BBox, PolyhedralCone)],
    nodes: &mut Vec<BvhNode>,
) -> usize {
    if order.len() == 1 {
        let fi = order[0];
        let (aabb, cone) = leaf_data[fi].clone();
        nodes.push(BvhNode {
            aabb,
            cone,
            payload: NodePayload::Leaf(fi),
        });
        return nodes.len() - 1;
    }

    let axis = longest_centroid_axis(order, leaf_data);
    order.sort_by(|&i, &j| {
        let ci = axis_coord(leaf_data[i].0.center(), axis);
        let cj = axis_coord(leaf_data[j].0.center(), axis);
        ci.partial_cmp(&cj).unwrap_or(std::cmp::Ordering::Equal)
    });

    let mid = order.len() / 2;
    let (left, right) = order.split_at_mut(mid);
    let li = build_subtree(left, leaf_data, nodes);
    let ri = build_subtree(right, leaf_data, nodes);

    let aabb = nodes[li].aabb.union(&nodes[ri].aabb);
    let cone = merge_cones(&nodes[li].cone, &nodes[ri].cone);
    nodes.push(BvhNode {
        aabb,
        cone,
        payload: NodePayload::Internal(li, ri),
    });
    nodes.len() - 1
}

/// Axis (0/1/2) along which the leaf centroids spread most.
fn longest_centroid_axis(order: &[usize], leaf_data: &[(BBox, PolyhedralCone)]) -> usize {
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for &i in order {
        let c = leaf_data[i].0.center();
        for (axis, &coord) in [c.x, c.y, c.z].iter().enumerate() {
            if coord < min[axis] {
                min[axis] = coord;
            }
            if coord > max[axis] {
                max[axis] = coord;
            }
        }
    }
    let spread = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let mut best = 0;
    for axis in 1..3 {
        if spread[axis] > spread[best] {
            best = axis;
        }
    }
    best
}

fn axis_coord(p: Point3, axis: usize) -> f64 {
    match axis {
        0 => p.x,
        1 => p.y,
        _ => p.z,
    }
}

/// Union of two cones: the conic hull of both generator sets.
fn merge_cones(a: &PolyhedralCone, b: &PolyhedralCone) -> PolyhedralCone {
    let mut gens = a.generators().to_vec();
    gens.extend_from_slice(b.generators());
    PolyhedralCone::from_generators(&gens)
}

/// Conservative AABB of a feature: from its faces' boundary vertices plus a
/// surface sample over each face's parameter extent.
fn feature_aabb(model: &BRepModel, feature: &SupermaximalFeature) -> BBox {
    let mut pts = Vec::new();
    for &fid in &feature.faces {
        collect_face_points(model, fid, &mut pts);
    }
    BBox::from_points(&pts).unwrap_or_else(|| BBox::new_validated(Vector3::ZERO, Vector3::ZERO))
}

/// Append boundary-vertex positions of `face_id` plus a grid sample of the
/// supporting surface over the face's projected parameter box to `out`.
fn collect_face_points(model: &BRepModel, face_id: FaceId, out: &mut Vec<Point3>) {
    let Some(face) = model.faces.get(face_id) else {
        return;
    };
    let Some(surface) = model.surfaces.get(face.surface_id) else {
        return;
    };
    let tol = model.tolerance();

    let mut uvs: Vec<(f64, f64)> = Vec::new();
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        let Some(lp) = model.loops.get(lid) else {
            continue;
        };
        for &eid in &lp.edges {
            let Some(edge) = model.edges.get(eid) else {
                continue;
            };
            for vid in [edge.start_vertex, edge.end_vertex] {
                if let Some(v) = model.vertices.get(vid) {
                    let p = Vector3::new(v.position[0], v.position[1], v.position[2]);
                    out.push(p);
                    if let Ok(uv) = surface.closest_point(&p, tol) {
                        uvs.push(uv);
                    }
                }
            }
        }
    }

    // Sample the surface across the projected-vertex parameter box so a curved
    // face's interior bulge is bounded, not just its boundary vertices.
    if uvs.len() >= 2 {
        let (mut u0, mut u1, mut v0, mut v1) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for &(u, v) in &uvs {
            u0 = u0.min(u);
            u1 = u1.max(u);
            v0 = v0.min(v);
            v1 = v1.max(v);
        }
        const N: usize = 5;
        for i in 0..N {
            let fu = i as f64 / (N - 1) as f64;
            let u = u0 + (u1 - u0) * fu;
            for j in 0..N {
                let fv = j as f64 / (N - 1) as f64;
                let v = v0 + (v1 - v0) * fv;
                if let Ok(p) = surface.point_at(u, v) {
                    out.push(p);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stateful broad phase with temporal coherence (CD-φ.6.2)
// ---------------------------------------------------------------------------

/// What the last traversal concluded about a node-pair on the BVTT *front* (the
/// frontier where traversal stopped — Crozet Sec 4.5.1).
#[derive(Debug, Clone, Copy)]
enum Outcome {
    /// A surviving leaf-pair: `(feature index in A, feature index in B)`.
    Candidate(usize, usize),
    /// A pruned node-pair (whole subtree below it is excluded).
    Culled,
}

#[derive(Debug, Clone, Copy)]
struct FrontPair {
    a: usize,
    b: usize,
    outcome: Outcome,
}

/// A **stateful** broad phase that exploits frame-to-frame coherence (Crozet
/// Sec 4.5). It persists two things across queries:
///
/// * a **separating-plane cache** per culled node-pair — re-testing a cached
///   plane is `O(generators)` dot-products, versus recomputing the full cone
///   intersection; and
/// * the **front** — the frontier of the last traversal. If every front pair
///   still classifies the same way (each cull still culled, each candidate still
///   a candidate), the candidate set is provably unchanged and is returned
///   without re-descending the trees.
///
/// Both are *hints*: a cached plane is always re-validated before use, and the
/// front fast-path falls back to a full traversal the moment anything crosses a
/// cull boundary. Correctness never depends on coherence — only speed does.
#[derive(Debug, Default)]
pub struct BroadPhase {
    front: Vec<FrontPair>,
    has_front: bool,
    sep_cache: HashMap<(usize, usize), Vector3>,
}

/// Outcome of a [`BroadPhase::query`], with the work it cost.
#[derive(Debug, Clone)]
pub struct BroadPhaseStats {
    /// Surviving `(feature index in A, feature index in B)` candidate pairs.
    pub pairs: Vec<(usize, usize)>,
    /// Node-pairs examined (front length on the warm path, traversal size cold).
    pub node_visits: usize,
    /// Full cone intersections computed (the expensive test).
    pub full_cone_tests: usize,
    /// Cone culls resolved cheaply by a cached separating plane.
    pub cached_plane_hits: usize,
    /// `true` if the warm front fast-path was taken (no tree re-descent).
    pub revalidated: bool,
}

impl BroadPhase {
    /// A fresh broad phase with no cached state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the surviving feature-pairs between two BVHs, reusing cached
    /// state from the previous query where it remains valid.
    pub fn query(&mut self, a: &FeatureBvh, b: &FeatureBvh) -> BroadPhaseStats {
        if self.has_front {
            if let Some(stats) = self.try_revalidate(a, b) {
                return stats;
            }
        }
        self.full_traversal(a, b)
    }

    /// Warm path: re-check every front pair. If all still classify identically,
    /// the candidate set is unchanged — return it without descending the trees.
    /// Returns `None` (forcing a full traversal) the moment a pair flips.
    fn try_revalidate(&mut self, a: &FeatureBvh, b: &FeatureBvh) -> Option<BroadPhaseStats> {
        let front = std::mem::take(&mut self.front);
        let mut pairs = Vec::new();
        let mut full_cone_tests = 0usize;
        let mut cached_plane_hits = 0usize;

        for fp in &front {
            let na = &a.nodes[fp.a];
            let nb = &b.nodes[fp.b];
            let overlap = na.aabb.intersects(&nb.aabb);
            match fp.outcome {
                Outcome::Candidate(fa, fb) => {
                    if !overlap {
                        return self.invalidate();
                    }
                    full_cone_tests += 1;
                    if matches!(
                        na.cone.intersects(&nb.cone.negated()),
                        ConeIntersectionResult::Disjoint { .. }
                    ) {
                        return self.invalidate();
                    }
                    pairs.push((fa, fb));
                }
                Outcome::Culled => {
                    if !overlap {
                        continue; // still apart spatially
                    }
                    let (used_cache, culled) = self.cone_cull((fp.a, fp.b), &na.cone, &nb.cone);
                    cached_plane_hits += used_cache as usize;
                    full_cone_tests += (!used_cache) as usize;
                    if !culled {
                        return self.invalidate();
                    }
                }
            }
        }

        let node_visits = front.len();
        self.front = front;
        self.has_front = true;
        Some(BroadPhaseStats {
            pairs,
            node_visits,
            full_cone_tests,
            cached_plane_hits,
            revalidated: true,
        })
    }

    /// Drop the stale front and signal the caller to fall back to a full
    /// traversal.
    fn invalidate(&mut self) -> Option<BroadPhaseStats> {
        self.has_front = false;
        None
    }

    /// Cold path: descend both trees, pruning by AABB then cone (with the cache),
    /// and rebuild the front.
    fn full_traversal(&mut self, a: &FeatureBvh, b: &FeatureBvh) -> BroadPhaseStats {
        let mut pairs = Vec::new();
        let mut frontier = Vec::new();
        let mut node_visits = 0usize;
        let mut full_cone_tests = 0usize;
        let mut cached_plane_hits = 0usize;

        if let (Some(ra), Some(rb)) = (a.root, b.root) {
            let mut stack = vec![(ra, rb)];
            while let Some((ai, bi)) = stack.pop() {
                node_visits += 1;
                let na = &a.nodes[ai];
                let nb = &b.nodes[bi];
                if !na.aabb.intersects(&nb.aabb) {
                    frontier.push(FrontPair {
                        a: ai,
                        b: bi,
                        outcome: Outcome::Culled,
                    });
                    continue;
                }
                let (used_cache, culled) = self.cone_cull((ai, bi), &na.cone, &nb.cone);
                cached_plane_hits += used_cache as usize;
                full_cone_tests += (!used_cache) as usize;
                if culled {
                    frontier.push(FrontPair {
                        a: ai,
                        b: bi,
                        outcome: Outcome::Culled,
                    });
                    continue;
                }
                match (na.payload, nb.payload) {
                    (NodePayload::Leaf(fa), NodePayload::Leaf(fb)) => {
                        pairs.push((fa, fb));
                        frontier.push(FrontPair {
                            a: ai,
                            b: bi,
                            outcome: Outcome::Candidate(fa, fb),
                        });
                    }
                    (NodePayload::Internal(l, r), _) => {
                        stack.push((l, bi));
                        stack.push((r, bi));
                    }
                    (NodePayload::Leaf(_), NodePayload::Internal(l, r)) => {
                        stack.push((ai, l));
                        stack.push((ai, r));
                    }
                }
            }
        }

        self.front = frontier;
        self.has_front = true;
        BroadPhaseStats {
            pairs,
            node_visits,
            full_cone_tests,
            cached_plane_hits,
            revalidated: false,
        }
    }

    /// Test whether a node-pair's cones are culled (can't oppose). Returns
    /// `(used_cache, culled)`: `used_cache` is `true` when a cached separating
    /// plane resolved it (no full intersection computed), so the caller counts a
    /// full cone test exactly when `!used_cache`. A fresh disjoint result caches
    /// its separating plane for next time.
    fn cone_cull(
        &mut self,
        key: (usize, usize),
        cone_a: &PolyhedralCone,
        cone_b: &PolyhedralCone,
    ) -> (bool, bool) {
        if let Some(p) = self.sep_cache.get(&key) {
            if plane_separates(*p, cone_a, cone_b) {
                return (true, true); // resolved by cache, still culled
            }
        }
        match cone_a.intersects(&cone_b.negated()) {
            ConeIntersectionResult::Overlapping => (false, false),
            ConeIntersectionResult::Disjoint { separating_plane } => {
                self.sep_cache.insert(key, separating_plane);
                (false, true)
            }
        }
    }
}

/// Does unit normal `p` still separate the two feature cones — every generator
/// of both on the `≤ 0` side? Then no normal of A opposes one of B and the pair
/// can't contact. `O(generators)`, the cheap temporal re-test.
fn plane_separates(p: Vector3, a: &PolyhedralCone, b: &PolyhedralCone) -> bool {
    const E: f64 = 1e-7;
    a.generators().iter().all(|g| g.dot(&p) <= E) && b.generators().iter().all(|g| g.dot(&p) <= E)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;
    const Z: Vector3 = Vector3::Z;

    /// A 2×2×2 box centred at the origin (corners ±1).
    fn box_at(model: &mut BRepModel) -> SolidId {
        TopologyBuilder::new(model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn box_bvh_has_six_leaves_and_is_well_formed() {
        let mut model = BRepModel::new();
        let solid = box_at(&mut model);
        let bvh = FeatureBvh::build(&model, solid);
        assert_eq!(bvh.feature_count(), 6, "6 box faces → 6 features");
        assert_eq!(
            bvh.node_count(),
            2 * 6 - 1,
            "full binary tree over 6 leaves"
        );
        assert!(
            bvh.well_formed(),
            "AABB + cone bounds conservative bottom-up"
        );
    }

    #[test]
    fn box_root_aabb_contains_the_box() {
        let mut model = BRepModel::new();
        let solid = box_at(&mut model);
        let bvh = FeatureBvh::build(&model, solid);
        let root = bvh.root_aabb().expect("root aabb");
        for &corner in &[
            Vector3::new(1.0, 1.0, 1.0),
            Vector3::new(-1.0, -1.0, -1.0),
            Vector3::new(1.0, -1.0, 0.0),
            Vector3::new(0.0, 0.0, 0.0),
        ] {
            assert!(root.contains_point(&corner), "root must contain {corner:?}");
        }
    }

    fn translate(model: &mut BRepModel, solid: SolidId, dist: f64) {
        crate::operations::transform::translate(model, vec![solid], X, dist, Default::default())
            .expect("translate");
    }

    #[test]
    fn separated_boxes_cull_to_nothing_in_one_visit() {
        let mut model = BRepModel::new();
        let a = box_at(&mut model);
        let b = box_at(&mut model);
        translate(&mut model, b, 10.0); // far apart
        let (bvh_a, bvh_b) = (FeatureBvh::build(&model, a), FeatureBvh::build(&model, b));
        let result = bvh_a.candidate_pairs(&bvh_b);
        assert!(result.pairs.is_empty(), "separated → no candidate pairs");
        assert_eq!(
            result.node_visits, 1,
            "root AABBs disjoint → prune immediately"
        );
    }

    #[test]
    fn touching_boxes_yield_consistent_candidates_subset_of_brute() {
        let mut model = BRepModel::new();
        let a = box_at(&mut model);
        let b = box_at(&mut model);
        translate(&mut model, b, 2.0); // faces meet at x = 1
        let (bvh_a, bvh_b) = (FeatureBvh::build(&model, a), FeatureBvh::build(&model, b));
        let result = bvh_a.candidate_pairs(&bvh_b);
        assert!(
            !result.pairs.is_empty(),
            "touching boxes share contact candidates"
        );
        // Soundness: the BVH only prunes — every surviving pair must itself pass
        // both the AABB and the cone test directly (no spurious pairs emitted).
        for &(fa, fb) in &result.pairs {
            let aabb_a = feature_aabb(&model, &bvh_a.features()[fa]);
            let aabb_b = feature_aabb(&model, &bvh_b.features()[fb]);
            assert!(
                aabb_a.intersects(&aabb_b),
                "emitted pair has disjoint AABBs"
            );
            let cone_a = feature_normal_cone(&model, &bvh_a.features()[fa]);
            let cone_b = feature_normal_cone(&model, &bvh_b.features()[fb]);
            assert!(
                features_can_contact(&cone_a, &cone_b),
                "emitted pair's cones cannot contact"
            );
        }
        // The pruned traversal never visits more node-pairs than the brute
        // leaf-pair product would (6 × 6 = 36), and emits no more than that.
        assert!(result.pairs.len() <= 36);
        assert!(result.node_visits <= 2 * 36, "traversal stays bounded");
    }

    #[test]
    fn empty_solid_builds_an_empty_bvh() {
        let model = BRepModel::new();
        // No solids → build on a bogus id yields an empty, queryable BVH.
        let bvh = FeatureBvh::build(&model, 0);
        assert_eq!(bvh.feature_count(), 0);
        assert!(bvh.root_aabb().is_none());
        let other = bvh.clone();
        assert!(bvh.candidate_pairs(&other).pairs.is_empty());
    }

    // -- φ.6.2 stateful broad phase ----------------------------------------

    fn sorted(mut v: Vec<(usize, usize)>) -> Vec<(usize, usize)> {
        v.sort_unstable();
        v
    }

    #[test]
    fn warm_query_revalidates_and_does_less_work() {
        let mut model = BRepModel::new();
        let a = box_at(&mut model);
        let b = box_at(&mut model);
        translate(&mut model, b, 2.0); // faces meet at x = 1
        let (bvh_a, bvh_b) = (FeatureBvh::build(&model, a), FeatureBvh::build(&model, b));

        let mut bp = BroadPhase::new();
        let cold = bp.query(&bvh_a, &bvh_b);
        assert!(!cold.revalidated, "first query is cold");
        assert!(!cold.pairs.is_empty());

        let warm = bp.query(&bvh_a, &bvh_b); // identical frame
        assert!(warm.revalidated, "stable frame takes the front fast-path");
        assert_eq!(
            sorted(warm.pairs.clone()),
            sorted(cold.pairs.clone()),
            "same candidates"
        );
        assert!(
            warm.node_visits <= cold.node_visits,
            "front ≤ full traversal"
        );
        assert!(
            warm.full_cone_tests < cold.full_cone_tests,
            "warm avoids most full cone intersections ({} vs {})",
            warm.full_cone_tests,
            cold.full_cone_tests
        );
        assert!(
            warm.cached_plane_hits > 0,
            "culled frontier resolved via cached planes"
        );
    }

    #[test]
    fn moving_a_body_invalidates_the_front_and_recomputes() {
        let mut model = BRepModel::new();
        let a = box_at(&mut model);
        let b = box_at(&mut model);
        translate(&mut model, b, 2.0); // touching
        let bvh_a = FeatureBvh::build(&model, a);
        let bvh_touch = FeatureBvh::build(&model, b);

        let mut bp = BroadPhase::new();
        let touch = bp.query(&bvh_a, &bvh_touch);
        assert!(!touch.pairs.is_empty());

        // B moves far away: same tree topology (deterministic build), new AABBs.
        let mut model2 = BRepModel::new();
        let a2 = box_at(&mut model2);
        let b2 = box_at(&mut model2);
        translate(&mut model2, b2, 50.0);
        let bvh_a2 = FeatureBvh::build(&model2, a2);
        let bvh_far = FeatureBvh::build(&model2, b2);

        // Reuse the SAME broad phase (its front is stale for the new positions).
        let moved = bp.query(&bvh_a2, &bvh_far);
        assert!(
            !moved.revalidated,
            "a candidate's AABBs separated → front invalid"
        );
        assert!(moved.pairs.is_empty(), "separated boxes have no candidates");
    }

    #[test]
    fn warm_query_preserves_correctness_against_cold() {
        // Whatever the warm path returns must equal a fresh cold traversal.
        let mut model = BRepModel::new();
        let a = box_at(&mut model);
        let b = box_at(&mut model);
        translate(&mut model, b, 2.0);
        let (bvh_a, bvh_b) = (FeatureBvh::build(&model, a), FeatureBvh::build(&model, b));

        let mut bp = BroadPhase::new();
        let _ = bp.query(&bvh_a, &bvh_b);
        let warm = bp.query(&bvh_a, &bvh_b);

        let mut fresh = BroadPhase::new();
        let cold = fresh.query(&bvh_a, &bvh_b);
        assert_eq!(
            sorted(warm.pairs),
            sorted(cold.pairs),
            "warm == cold result"
        );
    }
}
