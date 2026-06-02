//! Supermaximal feature grouping + a CD ablation harness (CD-φ.2.3).
//!
//! **Supermaximal grouping** (Crozet, *Smooth-BRep CD*, Sec 2.3): adjacent faces
//! that are the *same* canonical surface and meet across a *G1* edge are one CD
//! entity, not two. A cylinder split into halves by an imprinted ruling, or a
//! face split by a coplanar seam, should be reasoned about as a single feature —
//! merging them is what roughly halves the LMD pair count downstream (the cones
//! and the LMD engine then operate per *feature*, not per raw B-Rep face).
//!
//! The merge predicate has two guards, both necessary:
//! * the shared edge is **G1** (tangent-continuous) — a convex/concave crease is
//!   a real feature boundary and must not be crossed; and
//! * the two faces are the **same canonical surface** — same plane, same cylinder
//!   axis+radius, etc. Two coplanar faces merge; a fillet (cylinder) meeting a
//!   wall (plane) tangentially does *not*, even though their join is G1.
//!
//! **The ablation harness** ([`ablate_pairs`]) measures *why* this matters: it
//! reports the CD funnel — raw face-pairs → grouped feature-pairs → pairs that
//! survive cone culling — so each optimisation's contribution is a number, not a
//! claim. Grouping that stops merging, or culling that weakens, shows up as a
//! rising count.

use crate::math::polyhedral_cone::PolyhedralCone;
use crate::math::vector3::Vector3;
use crate::math::Tolerance;
use crate::operations::edge_classification::{classify_edge, find_adjacent_faces};
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::{Surface, SurfaceType};
use crate::primitives::topology_builder::BRepModel;
use crate::queries::cd::features_can_contact;
use std::collections::{HashMap, HashSet};

/// A maximal set of faces forming one CD entity: the same canonical surface,
/// connected across G1 edges. A single B-Rep face that joins nothing is its own
/// (singleton) feature.
#[derive(Debug, Clone)]
pub struct SupermaximalFeature {
    /// The B-Rep faces in this feature, sorted ascending.
    pub faces: Vec<FaceId>,
}

/// Partition a solid's faces into supermaximal features.
///
/// Two faces are merged iff they share a G1 edge **and** are the same canonical
/// surface with the same orientation. The result is a partition (every face in
/// exactly one feature), ordered by least face id for determinism.
pub fn supermaximal_features(model: &BRepModel, solid_id: SolidId) -> Vec<SupermaximalFeature> {
    let faces = solid_face_ids(model, solid_id);
    if faces.is_empty() {
        return Vec::new();
    }
    let face_set: HashSet<FaceId> = faces.iter().copied().collect();
    let mut parent: HashMap<FaceId, FaceId> = faces.iter().map(|&f| (f, f)).collect();

    for edge_id in solid_edge_ids(model, &faces) {
        let adj = find_adjacent_faces(model, edge_id);
        if adj.len() != 2 {
            continue; // boundary / non-manifold / seam (single face) — never merges
        }
        let (f1, f2) = (adj[0], adj[1]);
        if !face_set.contains(&f1) || !face_set.contains(&f2) {
            continue;
        }
        let g1 = classify_edge(model, edge_id)
            .map(|c| c.is_g1())
            .unwrap_or(false);
        if g1 && same_feature(model, f1, f2) {
            uf_union(&mut parent, f1, f2);
        }
    }

    let mut groups: HashMap<FaceId, Vec<FaceId>> = HashMap::new();
    for &f in &faces {
        let root = uf_find(&mut parent, f);
        groups.entry(root).or_default().push(f);
    }
    let mut out: Vec<SupermaximalFeature> = groups
        .into_values()
        .map(|mut fs| {
            fs.sort_unstable();
            SupermaximalFeature { faces: fs }
        })
        .collect();
    out.sort_unstable_by_key(|f| f.faces[0]);
    out
}

/// The normal cone of a feature: the conic hull of the outward normals of all
/// its faces (sampled at each face's parameter-domain centre — exact for the
/// canonical surfaces, representative for free-form). For a smooth feature
/// spanning curvature, [`PolyhedralCone::dilate`] widens this to cover the
/// normal's variation.
pub fn feature_normal_cone(model: &BRepModel, feature: &SupermaximalFeature) -> PolyhedralCone {
    let normals: Vec<Vector3> = feature
        .faces
        .iter()
        .filter_map(|&f| face_outward_normal(model, f))
        .collect();
    PolyhedralCone::from_generators(&normals)
}

/// The CD funnel measured between two solids — the ablation result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdAblation {
    /// Pairs if every raw B-Rep face is its own CD entity (no grouping).
    pub raw_face_pairs: usize,
    /// Pairs after supermaximal grouping (the grouping ablation).
    pub feature_pairs: usize,
    /// Feature-pairs that survive cone culling (the culling ablation).
    pub cone_surviving_pairs: usize,
}

impl CdAblation {
    /// Fraction of raw face-pairs left after grouping (lower = grouping helped).
    pub fn grouping_fraction(&self) -> f64 {
        ratio(self.feature_pairs, self.raw_face_pairs)
    }
    /// Fraction of feature-pairs left after cone culling (lower = culling helped).
    pub fn culling_fraction(&self) -> f64 {
        ratio(self.cone_surviving_pairs, self.feature_pairs)
    }
    /// Fraction of raw face-pairs that survive the whole funnel.
    pub fn total_fraction(&self) -> f64 {
        ratio(self.cone_surviving_pairs, self.raw_face_pairs)
    }
}

fn ratio(num: usize, den: usize) -> f64 {
    if den == 0 {
        0.0
    } else {
        num as f64 / den as f64
    }
}

/// Run the CD ablation between two solids: count raw face-pairs, grouped
/// feature-pairs, and feature-pairs surviving cone culling. `cone_dilation` is
/// the half-angle (radians) each feature's normal cone is widened by before the
/// cull — `0.0` for the sharpest (canonical-exact) measurement, a few degrees to
/// stay conservative over curved features.
pub fn ablate_pairs(
    model: &BRepModel,
    solid_a: SolidId,
    solid_b: SolidId,
    cone_dilation: f64,
) -> CdAblation {
    let faces_a = solid_face_ids(model, solid_a);
    let faces_b = solid_face_ids(model, solid_b);
    let feats_a = supermaximal_features(model, solid_a);
    let feats_b = supermaximal_features(model, solid_b);

    let cones_a: Vec<PolyhedralCone> = feats_a
        .iter()
        .map(|f| feature_normal_cone(model, f).dilate(cone_dilation))
        .collect();
    let cones_b: Vec<PolyhedralCone> = feats_b
        .iter()
        .map(|f| feature_normal_cone(model, f).dilate(cone_dilation))
        .collect();

    let mut surviving = 0;
    for ca in &cones_a {
        for cb in &cones_b {
            if features_can_contact(ca, cb) {
                surviving += 1;
            }
        }
    }

    CdAblation {
        raw_face_pairs: faces_a.len() * faces_b.len(),
        feature_pairs: feats_a.len() * feats_b.len(),
        cone_surviving_pairs: surviving,
    }
}

/// Are two faces the same canonical surface with the same orientation — the
/// geometric half of the supermaximal merge test?
pub fn same_feature(model: &BRepModel, f1: FaceId, f2: FaceId) -> bool {
    let (Some(fa), Some(fb)) = (model.faces.get(f1), model.faces.get(f2)) else {
        return false;
    };
    if fa.orientation != fb.orientation {
        return false;
    }
    let (Some(sa), Some(sb)) = (
        model.surfaces.get(fa.surface_id),
        model.surfaces.get(fb.surface_id),
    ) else {
        return false;
    };
    same_canonical_surface(sa, sb, model.tolerance())
}

/// Are two surfaces the *same* canonical surface — same kind and same defining
/// parameters (plane, cylinder axis+radius, sphere centre+radius, …) within
/// tolerance? Free-form (NURBS / Bézier) surfaces are never reported equal here.
pub fn same_canonical_surface(a: &dyn Surface, b: &dyn Surface, tol: Tolerance) -> bool {
    use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Torus};
    let pos = tol.distance().max(1e-6);
    const DIR: f64 = 1e-6;
    match (a.surface_type(), b.surface_type()) {
        (SurfaceType::Plane, SurfaceType::Plane) => {
            let (Some(p), Some(q)) = (dc::<Plane>(a), dc::<Plane>(b)) else {
                return false;
            };
            same_dir(p.normal, q.normal, DIR) && (q.origin - p.origin).dot(&p.normal).abs() < pos
        }
        (SurfaceType::Cylinder, SurfaceType::Cylinder) => {
            let (Some(p), Some(q)) = (dc::<Cylinder>(a), dc::<Cylinder>(b)) else {
                return false;
            };
            same_axis(p.axis, q.axis, DIR)
                && (p.radius - q.radius).abs() < pos
                && perp_dist(p.origin, q.origin, p.axis) < pos
        }
        (SurfaceType::Sphere, SurfaceType::Sphere) => {
            let (Some(p), Some(q)) = (dc::<Sphere>(a), dc::<Sphere>(b)) else {
                return false;
            };
            (p.center - q.center).magnitude() < pos && (p.radius - q.radius).abs() < pos
        }
        (SurfaceType::Cone, SurfaceType::Cone) => {
            let (Some(p), Some(q)) = (dc::<Cone>(a), dc::<Cone>(b)) else {
                return false;
            };
            (p.apex - q.apex).magnitude() < pos
                && same_axis(p.axis, q.axis, DIR)
                && (p.half_angle - q.half_angle).abs() < 1e-6
        }
        (SurfaceType::Torus, SurfaceType::Torus) => {
            let (Some(p), Some(q)) = (dc::<Torus>(a), dc::<Torus>(b)) else {
                return false;
            };
            (p.center - q.center).magnitude() < pos
                && same_axis(p.axis, q.axis, DIR)
                && (p.major_radius - q.major_radius).abs() < pos
                && (p.minor_radius - q.minor_radius).abs() < pos
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// helpers (private)
// ---------------------------------------------------------------------------

fn dc<T: std::any::Any>(s: &dyn Surface) -> Option<&T> {
    s.as_any().downcast_ref::<T>()
}

/// Same unit direction (sign-sensitive).
fn same_dir(a: Vector3, b: Vector3, eps: f64) -> bool {
    (a - b).magnitude() < eps
}

/// Same axis *line* (parallel, either sign).
fn same_axis(a: Vector3, b: Vector3, eps: f64) -> bool {
    (1.0 - a.dot(&b).abs()) < eps
}

/// Distance between two points measured perpendicular to `axis`.
fn perp_dist(o1: Vector3, o2: Vector3, axis: Vector3) -> f64 {
    let d = o1 - o2;
    (d - axis * d.dot(&axis)).magnitude()
}

/// Outward unit normal of a face, sampled at its parameter-domain centre.
fn face_outward_normal(model: &BRepModel, face_id: FaceId) -> Option<Vector3> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    let [u0, u1, v0, v1] = face.uv_bounds;
    let n = surface
        .normal_at(0.5 * (u0 + u1), 0.5 * (v0 + v1))
        .ok()?
        .normalize()
        .ok()?;
    Some(n * face.orientation.sign())
}

fn solid_face_ids(model: &BRepModel, solid_id: SolidId) -> Vec<FaceId> {
    let mut out = Vec::new();
    let Some(solid) = model.solids.get(solid_id) else {
        return out;
    };
    let mut shell_ids = vec![solid.outer_shell];
    shell_ids.extend(solid.inner_shells.iter().copied());
    for sid in shell_ids {
        if let Some(shell) = model.shells.get(sid) {
            out.extend(shell.faces.iter().copied());
        }
    }
    out
}

fn solid_edge_ids(model: &BRepModel, faces: &[FaceId]) -> Vec<EdgeId> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for &fid in faces {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let mut loop_ids = vec![face.outer_loop];
        loop_ids.extend(face.inner_loops.iter().copied());
        for lid in loop_ids {
            if let Some(lp) = model.loops.get(lid) {
                for &e in &lp.edges {
                    if seen.insert(e) {
                        out.push(e);
                    }
                }
            }
        }
    }
    out
}

fn uf_find(parent: &mut HashMap<FaceId, FaceId>, x: FaceId) -> FaceId {
    let mut root = x;
    while parent[&root] != root {
        root = parent[&root];
    }
    let mut cur = x;
    while parent[&cur] != root {
        let next = parent[&cur];
        parent.insert(cur, root);
        cur = next;
    }
    root
}

fn uf_union(parent: &mut HashMap<FaceId, FaceId>, a: FaceId, b: FaceId) {
    let ra = uf_find(parent, a);
    let rb = uf_find(parent, b);
    if ra != rb {
        parent.insert(ra, rb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Point3;
    use crate::primitives::curve::{Line, ParameterRange};
    use crate::primitives::edge::{Edge, EdgeOrientation};
    use crate::primitives::face::{Face, FaceOrientation};
    use crate::primitives::r#loop::{Loop, LoopType};
    use crate::primitives::shell::{Shell, ShellType};
    use crate::primitives::solid::Solid;
    use crate::primitives::surface::{Cone, Cylinder, Plane, Sphere, Torus};
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;
    const Y: Vector3 = Vector3::Y;
    const Z: Vector3 = Vector3::Z;

    fn t() -> Tolerance {
        Tolerance::default()
    }

    fn plane(o: Point3, n: Vector3) -> Plane {
        let seed = if n.dot(&X).abs() < 0.9 { X } else { Y };
        Plane::new(o, n, seed).expect("plane")
    }

    // -- same_canonical_surface ---------------------------------------------

    #[test]
    fn identical_planes_match_different_ones_dont() {
        let a = plane(Vector3::new(0.0, 0.0, 0.0), Z);
        let same = plane(Vector3::new(3.0, -2.0, 0.0), Z); // same z=0 plane, different origin in-plane
        let shifted = plane(Vector3::new(0.0, 0.0, 1.0), Z); // parallel but offset
        let tilted = plane(Vector3::new(0.0, 0.0, 0.0), X); // different normal
        assert!(same_canonical_surface(&a, &same, t()));
        assert!(!same_canonical_surface(&a, &shifted, t()));
        assert!(!same_canonical_surface(&a, &tilted, t()));
    }

    #[test]
    fn cylinders_match_on_axis_line_and_radius() {
        let a = Cylinder::new(Vector3::new(0.0, 0.0, 0.0), Z, 2.0).expect("cyl");
        let same = Cylinder::new(Vector3::new(0.0, 0.0, 5.0), Z, 2.0).expect("cyl"); // same axis line, slid along it
        let fatter = Cylinder::new(Vector3::new(0.0, 0.0, 0.0), Z, 2.5).expect("cyl");
        let offset = Cylinder::new(Vector3::new(3.0, 0.0, 0.0), Z, 2.0).expect("cyl"); // parallel, different line
        assert!(same_canonical_surface(&a, &same, t()));
        assert!(!same_canonical_surface(&a, &fatter, t()));
        assert!(!same_canonical_surface(&a, &offset, t()));
    }

    #[test]
    fn spheres_and_mixed_kinds() {
        let s1 = Sphere::new(Vector3::new(1.0, 1.0, 1.0), 2.0).expect("s");
        let s2 = Sphere::new(Vector3::new(1.0, 1.0, 1.0), 2.0).expect("s");
        let s3 = Sphere::new(Vector3::new(1.0, 1.0, 1.0), 2.1).expect("s");
        assert!(same_canonical_surface(&s1, &s2, t()));
        assert!(!same_canonical_surface(&s1, &s3, t()));
        // mixed kind never matches
        let pl = plane(Vector3::new(0.0, 0.0, 0.0), Z);
        assert!(!same_canonical_surface(&s1, &pl, t()));
    }

    #[test]
    fn cones_and_tori() {
        let c1 = Cone::new(Vector3::new(0.0, 0.0, 0.0), Z, 0.5).expect("cone");
        let c2 = Cone::new(Vector3::new(0.0, 0.0, 0.0), Z, 0.5).expect("cone");
        let c3 = Cone::new(Vector3::new(0.0, 0.0, 0.0), Z, 0.6).expect("cone");
        assert!(same_canonical_surface(&c1, &c2, t()));
        assert!(!same_canonical_surface(&c1, &c3, t()));

        let to1 = Torus::new(Vector3::new(0.0, 0.0, 0.0), Z, 3.0, 1.0).expect("torus");
        let to2 = Torus::new(Vector3::new(0.0, 0.0, 0.0), Z, 3.0, 1.0).expect("torus");
        let to3 = Torus::new(Vector3::new(0.0, 0.0, 0.0), Z, 3.0, 1.2).expect("torus");
        assert!(same_canonical_surface(&to1, &to2, t()));
        assert!(!same_canonical_surface(&to1, &to3, t()));
    }

    // -- grouping on real solids -------------------------------------------

    fn box_solid(model: &mut BRepModel) -> SolidId {
        TopologyBuilder::new(model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn box_has_six_singleton_features() {
        // All 12 edges of a box are convex (90°), never G1 → no merges.
        let mut model = BRepModel::new();
        let solid = box_solid(&mut model);
        let feats = supermaximal_features(&model, solid);
        assert_eq!(feats.len(), 6, "box → 6 features");
        assert!(feats.iter().all(|f| f.faces.len() == 1), "no merges");
        // partition: every face covered exactly once.
        let total: usize = feats.iter().map(|f| f.faces.len()).sum();
        assert_eq!(total, 6);
    }

    #[test]
    fn cylinder_has_three_features() {
        // Lateral face + 2 caps; the cap↔lateral edges are convex → 3 features.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_cylinder_3d(Vector3::new(0.0, 0.0, 0.0), Z, 1.0, 2.0)
            .expect("cylinder");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("solid");
        let feats = supermaximal_features(&model, solid);
        assert_eq!(feats.len(), 3, "lateral + 2 caps, none merged");
    }

    /// Two coplanar quads sharing one edge — both on the z=0 plane (distinct
    /// `Plane` surfaces with equal parameters), joined across a flat (G1) edge.
    /// Returns the model and its solid id.
    fn two_coplanar_quads() -> (BRepModel, SolidId, EdgeId) {
        let mut model = BRepModel::new();
        let tol = model.tolerance().distance();
        let v00 = model.vertices.add_or_find(0.0, 0.0, 0.0, tol);
        let v10 = model.vertices.add_or_find(1.0, 0.0, 0.0, tol);
        let v11 = model.vertices.add_or_find(1.0, 1.0, 0.0, tol);
        let v01 = model.vertices.add_or_find(0.0, 1.0, 0.0, tol);
        let v20 = model.vertices.add_or_find(2.0, 0.0, 0.0, tol);
        let v21 = model.vertices.add_or_find(2.0, 1.0, 0.0, tol);
        let p = |x: f64, y: f64| Vector3::new(x, y, 0.0);

        // Closure captures nothing (model is passed in), so it can run repeatedly.
        let add_edge = |model: &mut BRepModel, sv: u32, ev: u32, a: Point3, b: Point3| -> EdgeId {
            let cid = model.curves.add(Box::new(Line::new(a, b)));
            model.edges.add(Edge::new(
                0,
                sv,
                ev,
                cid,
                EdgeOrientation::Forward,
                ParameterRange::new(0.0, 1.0),
            ))
        };

        let e_a_bottom = add_edge(&mut model, v00, v10, p(0.0, 0.0), p(1.0, 0.0));
        let e_shared = add_edge(&mut model, v10, v11, p(1.0, 0.0), p(1.0, 1.0));
        let e_a_top = add_edge(&mut model, v11, v01, p(1.0, 1.0), p(0.0, 1.0));
        let e_a_left = add_edge(&mut model, v01, v00, p(0.0, 1.0), p(0.0, 0.0));
        let e_b_bottom = add_edge(&mut model, v10, v20, p(1.0, 0.0), p(2.0, 0.0));
        let e_b_right = add_edge(&mut model, v20, v21, p(2.0, 0.0), p(2.0, 1.0));
        let e_b_top = add_edge(&mut model, v21, v11, p(2.0, 1.0), p(1.0, 1.0));

        let mut loop_a = Loop::new(0, LoopType::Outer);
        loop_a.add_edge(e_a_bottom, true);
        loop_a.add_edge(e_shared, true);
        loop_a.add_edge(e_a_top, true);
        loop_a.add_edge(e_a_left, true);
        let loop_a_id = model.loops.add(loop_a);

        let mut loop_b = Loop::new(0, LoopType::Outer);
        loop_b.add_edge(e_b_bottom, true);
        loop_b.add_edge(e_b_right, true);
        loop_b.add_edge(e_b_top, true);
        loop_b.add_edge(e_shared, false); // shared edge traversed in reverse for B
        let loop_b_id = model.loops.add(loop_b);

        // Distinct Plane surfaces with identical parameters (mimics split faces).
        let surf_a = model
            .surfaces
            .add(Box::new(plane(Vector3::new(0.0, 0.0, 0.0), Z)));
        let surf_b = model
            .surfaces
            .add(Box::new(plane(Vector3::new(0.0, 0.0, 0.0), Z)));
        let face_a = model
            .faces
            .add(Face::new(0, surf_a, loop_a_id, FaceOrientation::Forward));
        let face_b = model
            .faces
            .add(Face::new(0, surf_b, loop_b_id, FaceOrientation::Forward));

        let mut shell = Shell::new(0, ShellType::Open);
        shell.faces = vec![face_a, face_b];
        let shell_id = model.shells.add(shell);
        let solid_id = model.solids.add(Solid::new(0, shell_id));
        (model, solid_id, e_shared)
    }

    #[test]
    fn coplanar_faces_merge_into_one_feature() {
        let (model, solid, e_shared) = two_coplanar_quads();
        // Preconditions: the shared edge is seen by both faces and is G1 — if
        // these fail, it's the fixture, not the grouping logic.
        assert_eq!(
            find_adjacent_faces(&model, e_shared).len(),
            2,
            "fixture: shared edge must border both faces"
        );
        assert!(
            classify_edge(&model, e_shared)
                .map(|c| c.is_g1())
                .unwrap_or(false),
            "fixture: coplanar join must classify G1"
        );
        let feats = supermaximal_features(&model, solid);
        assert_eq!(feats.len(), 1, "two coplanar faces → one feature");
        assert_eq!(feats[0].faces.len(), 2);
    }

    // -- ablation harness ---------------------------------------------------

    #[test]
    fn ablation_cones_cull_box_box_to_opposed_faces() {
        // Two separated boxes. No grouping (all convex edges) → 36 raw = 36
        // feature pairs. With zero dilation each face's normal cone is a single
        // ray, so a pair survives the cull iff the normals are exactly opposed
        // (n_a = −n_b): each of A's 6 faces opposes exactly one of B's → 6.
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(&mut model, vec![b], X, 10.0, Default::default())
            .expect("translate B clear of A");

        let abl = ablate_pairs(&model, a, b, 0.0);
        assert_eq!(abl.raw_face_pairs, 36, "6 × 6 faces");
        assert_eq!(abl.feature_pairs, 36, "boxes don't group");
        assert_eq!(abl.cone_surviving_pairs, 6, "only opposed faces survive");
        assert!(abl.culling_fraction() < 0.2, "cones cull > 80%");
    }

    #[test]
    fn ablation_grouping_shrinks_the_pair_count() {
        // Coplanar fixture (2 faces → 1 feature) vs a box: grouping cuts A's
        // contribution in half, so feature_pairs < raw_face_pairs.
        let (mut model, a, _e) = two_coplanar_quads();
        let b = box_solid(&mut model);
        let abl = ablate_pairs(&model, a, b, 0.0);
        assert_eq!(abl.raw_face_pairs, 2 * 6);
        assert_eq!(
            abl.feature_pairs,
            1 * 6,
            "A's two faces merged to one feature"
        );
        assert!(
            abl.grouping_fraction() < 1.0,
            "grouping reduced the pair count"
        );
    }
}
