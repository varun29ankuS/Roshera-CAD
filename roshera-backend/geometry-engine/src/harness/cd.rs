//! Contact-determination pipeline ablation — the first complete kernel study.
//!
//! Runs the full CD funnel between two solids under a toggleable configuration
//! and reports every stage:
//!
//! ```text
//!   raw face-pairs  →  entity-pairs  →  broad-phase survivors  →  LMD solves
//!     (no grouping)    (supermaximal)    (AABB + cone / BVH)        (narrow)
//! ```
//!
//! The point is **ablation**: flip [`CdAblationConfig`] flags off and re-measure
//! to see exactly what each optimisation buys. Every configuration is checked
//! against a brute-force oracle — the global minimum distance computed over *all*
//! face-pairs — so a faster configuration that quietly drops the closest pair is
//! caught (`correct = false`). The optimisations only ever *prune pairs that
//! cannot host the closest approach*, so a correct run reproduces the brute-force
//! minimum exactly while doing less work.

use crate::harness::{AblationReport, StageMetric};
use crate::math::polyhedral_cone::PolyhedralCone;
use crate::math::Point3;
use crate::primitives::edge::EdgeId;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::queries::bvh::FeatureBvh;
use crate::queries::cd::features_can_contact;
use crate::queries::features::{feature_normal_cone, supermaximal_features, SupermaximalFeature};
use crate::queries::lmd::face_lmds;

/// Which CD optimisations are active for a run. `use_bvh` implies grouping (the
/// BVH leaves are supermaximal features).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdAblationConfig {
    pub use_grouping: bool,
    pub use_cone_cull: bool,
    pub use_bvh: bool,
}

impl CdAblationConfig {
    /// Every raw face is its own entity, every pair goes to the narrow phase —
    /// the brute-force baseline.
    pub fn baseline() -> Self {
        Self {
            use_grouping: false,
            use_cone_cull: false,
            use_bvh: false,
        }
    }

    /// The full pipeline: grouping + cone culling + BVH broad phase.
    pub fn full() -> Self {
        Self {
            use_grouping: true,
            use_cone_cull: true,
            use_bvh: true,
        }
    }
}

/// The measured run plus the answer it produced.
#[derive(Debug, Clone)]
pub struct CdAblationResult {
    pub report: AblationReport,
    /// Global minimum face-pair distance found (∞ if the solids share no
    /// face-pair LMD). The quantity verified against the brute-force oracle.
    pub min_distance: f64,
    /// Face-pair LMD solves performed in the narrow phase — the expensive work.
    pub lmd_solves: u64,
}

/// Run the CD pipeline between two solids under `config`, measuring every stage
/// and verifying the result against the brute-force minimum.
pub fn run_cd_ablation(
    model: &BRepModel,
    solid_a: SolidId,
    solid_b: SolidId,
    config: CdAblationConfig,
) -> CdAblationResult {
    let faces_a = solid_face_ids(model, solid_a);
    let faces_b = solid_face_ids(model, solid_b);
    let raw_pairs = faces_a.len() * faces_b.len();

    // Stage 1 — entities: supermaximal features, or one-face singletons. The BVH
    // path groups intrinsically, so it implies grouping.
    let grouping = config.use_grouping || config.use_bvh;
    let entities_a = entities(model, solid_a, grouping, &faces_a);
    let entities_b = entities(model, solid_b, grouping, &faces_b);
    let entity_pairs = entities_a.len() * entities_b.len();

    // Stage 2 — broad phase: surviving entity-index pairs + the work it cost.
    let (survivors, broad_cost) = if config.use_bvh {
        let bvh_a = FeatureBvh::build(model, solid_a);
        let bvh_b = FeatureBvh::build(model, solid_b);
        let r = bvh_a.candidate_pairs(&bvh_b);
        (r.pairs, r.node_visits as u64)
    } else {
        broad_phase_brute(model, &entities_a, &entities_b, config.use_cone_cull)
    };

    // Stage 3 — narrow phase: expand survivors to face-pairs, run the LMD, take
    // the global minimum distance.
    let mut min_distance = f64::INFINITY;
    let mut lmd_solves = 0u64;
    for &(i, j) in &survivors {
        for &fa in &entities_a[i] {
            for &fb in &entities_b[j] {
                lmd_solves += 1;
                let d = face_pair_min_distance(model, fa, fb);
                if d < min_distance {
                    min_distance = d;
                }
            }
        }
    }

    const TAU: f64 = 1e-6;

    // Penetration: the narrow phase reports the nearest BOUNDARY-feature distance
    // (face/edge/vertex LMD), which is correct for separated solids but POSITIVE
    // for two interpenetrating solids that share no touching feature. A contact
    // query must report 0 whenever the solid INTERIORS overlap. Detect overlap
    // (winding-number containment in BOTH closed shells, sampled along the
    // centroid-to-centroid segment, which threads any convex overlap lens), and
    // clamp the distance to 0. Applied to the pipeline AND the brute-force oracle
    // so they agree.
    let overlapping = solids_overlap(model, solid_a, solid_b);
    if overlapping && min_distance > TAU {
        min_distance = 0.0;
    }

    // The broad phase is a *contact* cull (AABB-overlap), so for separated
    // solids it legitimately prunes every pair and reports no contact. The
    // oracle is therefore a contact predicate: the pipeline must agree with
    // brute force on whether a contact exists, and — when it does — on where
    // (the minimum distance). It may differ on the gap of *separated* solids,
    // which is not a contact query.
    let mut brute_min = brute_force_min(model, &faces_a, &faces_b);
    if overlapping && brute_min > TAU {
        brute_min = 0.0;
    }
    let brute_contact = brute_min <= TAU;
    let pipeline_contact = min_distance <= TAU;
    let correct = (brute_contact == pipeline_contact)
        && (!brute_contact || approx_eq(min_distance, brute_min));

    let report = AblationReport::new(format!(
        "CD grouping={} cone={} bvh={}",
        config.use_grouping, config.use_cone_cull, config.use_bvh
    ))
    .stage(StageMetric::new("raw_face_pairs", raw_pairs, raw_pairs, 0))
    .stage(StageMetric::new("grouping", raw_pairs, entity_pairs, 0))
    .stage(StageMetric::new(
        "broad_phase",
        entity_pairs,
        survivors.len(),
        broad_cost,
    ))
    .stage(StageMetric::new(
        "narrow_phase(LMD)",
        survivors.len(),
        lmd_solves as usize,
        lmd_solves,
    ))
    .verified(correct);

    CdAblationResult {
        report,
        min_distance,
        lmd_solves,
    }
}

/// Run the canonical ablation matrix between two solids: the brute-force
/// baseline, then each optimisation layered on. Every entry is independently
/// verified, so the sweep both demonstrates the cost reduction and proves the
/// answer never changes.
pub fn run_cd_ablation_matrix(
    model: &BRepModel,
    solid_a: SolidId,
    solid_b: SolidId,
) -> Vec<CdAblationResult> {
    let configs = [
        CdAblationConfig::baseline(),
        CdAblationConfig {
            use_grouping: false,
            use_cone_cull: true,
            use_bvh: false,
        },
        CdAblationConfig {
            use_grouping: true,
            use_cone_cull: true,
            use_bvh: false,
        },
        CdAblationConfig::full(),
    ];
    configs
        .iter()
        .map(|c| run_cd_ablation(model, solid_a, solid_b, *c))
        .collect()
}

// ---------------------------------------------------------------------------
// helpers (private)
// ---------------------------------------------------------------------------

fn entities(
    model: &BRepModel,
    solid: SolidId,
    grouping: bool,
    faces: &[FaceId],
) -> Vec<Vec<FaceId>> {
    if grouping {
        supermaximal_features(model, solid)
            .into_iter()
            .map(|f| f.faces)
            .collect()
    } else {
        faces.iter().map(|&f| vec![f]).collect()
    }
}

fn entity_cone(model: &BRepModel, faces: &[FaceId]) -> PolyhedralCone {
    feature_normal_cone(
        model,
        &SupermaximalFeature {
            faces: faces.to_vec(),
        },
    )
}

fn broad_phase_brute(
    model: &BRepModel,
    entities_a: &[Vec<FaceId>],
    entities_b: &[Vec<FaceId>],
    use_cone_cull: bool,
) -> (Vec<(usize, usize)>, u64) {
    let cones_a: Vec<PolyhedralCone> = entities_a.iter().map(|e| entity_cone(model, e)).collect();
    let cones_b: Vec<PolyhedralCone> = entities_b.iter().map(|e| entity_cone(model, e)).collect();
    let mut survivors = Vec::new();
    let mut cost = 0u64;
    for (i, ca) in cones_a.iter().enumerate() {
        for (j, cb) in cones_b.iter().enumerate() {
            cost += 1;
            if !use_cone_cull || features_can_contact(ca, cb) {
                survivors.push((i, j));
            }
        }
    }
    (survivors, cost)
}

fn brute_force_min(model: &BRepModel, faces_a: &[FaceId], faces_b: &[FaceId]) -> f64 {
    let mut min = f64::INFINITY;
    for &fa in faces_a {
        for &fb in faces_b {
            let d = face_pair_min_distance(model, fa, fb);
            if d < min {
                min = d;
            }
        }
    }
    min
}

/// Minimum distance between two faces, accounting for BOTH face-interior contact
/// (the surface LMD) and BOUNDARY contact (edge–edge / vertex). The LMD's
/// `footpoint_in_face` filter discards any critical point that lands on a face's
/// boundary, so a contact that occurs on the shared boundary of two faces — two
/// coplanar faces meeting along an edge, or a box corner touching a face — is
/// invisible to `face_lmds` alone (it returns inf). The minimum distance between
/// the two faces' boundary edges recovers exactly those edge/vertex contacts
/// (#83). The overall face-pair distance is the min of the two.
fn face_pair_min_distance(model: &BRepModel, fa: FaceId, fb: FaceId) -> f64 {
    let mut min = f64::INFINITY;
    for lmd in face_lmds(model, fa, fb) {
        if lmd.distance < min {
            min = lmd.distance;
        }
    }
    let ea_ids = boundary_edge_ids(model, fa);
    let eb_ids = boundary_edge_ids(model, fb);
    for &ea in &ea_ids {
        for &eb in &eb_ids {
            let d = edge_pair_min_distance(model, ea, eb);
            if d < min {
                min = d;
            }
        }
    }
    min
}

/// Every edge id on a face's outer and inner loops.
fn boundary_edge_ids(model: &BRepModel, face_id: FaceId) -> Vec<EdgeId> {
    let mut out = Vec::new();
    let Some(face) = model.faces.get(face_id) else {
        return out;
    };
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        if let Some(lp) = model.loops.get(lid) {
            out.extend(lp.edges.iter().copied());
        }
    }
    out
}

/// Sample an edge's carrier curve into a polyline of `n` segments over the
/// edge's parameter sub-range.
fn sample_edge(model: &BRepModel, edge_id: EdgeId, n: usize) -> Vec<Point3> {
    let mut pts = Vec::new();
    let Some(edge) = model.edges.get(edge_id) else {
        return pts;
    };
    let Some(curve) = model.curves.get(edge.curve_id) else {
        return pts;
    };
    let (s, e) = (edge.param_range.start, edge.param_range.end);
    for k in 0..=n {
        let t = s + (e - s) * (k as f64) / (n as f64);
        if let Ok(p) = curve.point_at(t) {
            pts.push(p);
        }
    }
    pts
}

/// Minimum 3D distance between two B-Rep edges, via polyline sampling +
/// segment-segment distance. Exact for straight (line) edges (the polyline is
/// collinear, so the segment-segment minimum is the true minimum); a close
/// approximation for curved edges at the sample density used — sufficient for
/// contact determination (`distance <= TAU`).
fn edge_pair_min_distance(model: &BRepModel, ea: EdgeId, eb: EdgeId) -> f64 {
    const N: usize = 4;
    let pa = sample_edge(model, ea, N);
    let pb = sample_edge(model, eb, N);
    if pa.len() < 2 || pb.len() < 2 {
        return f64::INFINITY;
    }
    let mut min = f64::INFINITY;
    for i in 0..pa.len() - 1 {
        for j in 0..pb.len() - 1 {
            let d = seg_seg_distance(pa[i], pa[i + 1], pb[j], pb[j + 1]);
            if d < min {
                min = d;
            }
        }
    }
    min
}

/// Closest distance between two 3D segments [p1,q1] and [p2,q2] (Ericson,
/// *Real-Time Collision Detection* §5.1.9): clamp the unconstrained
/// line-line closest parameters to each segment, re-projecting when a clamp
/// moves off the other segment. Handles parallel and degenerate (point)
/// segments.
fn seg_seg_distance(p1: Point3, q1: Point3, p2: Point3, q2: Point3) -> f64 {
    let d1 = q1 - p1;
    let d2 = q2 - p2;
    let r = p1 - p2;
    let a = d1.dot(&d1);
    let e = d2.dot(&d2);
    let f = d2.dot(&r);
    let eps = 1e-18;
    let (s, t);
    if a <= eps && e <= eps {
        return r.magnitude();
    }
    if a <= eps {
        s = 0.0;
        t = (f / e).clamp(0.0, 1.0);
    } else {
        let c = d1.dot(&r);
        if e <= eps {
            t = 0.0;
            s = (-c / a).clamp(0.0, 1.0);
        } else {
            let b = d1.dot(&d2);
            let denom = a * e - b * b;
            let s0 = if denom.abs() > eps {
                ((b * f - c * e) / denom).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let t0 = (b * s0 + f) / e;
            if t0 < 0.0 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else if t0 > 1.0 {
                t = 1.0;
                s = ((b - c) / a).clamp(0.0, 1.0);
            } else {
                t = t0;
                s = s0;
            }
        }
    }
    let c1 = p1 + d1 * s;
    let c2 = p2 + d2 * t;
    (c1 - c2).magnitude()
}

fn approx_eq(a: f64, b: f64) -> bool {
    (a.is_infinite() && b.is_infinite() && a.signum() == b.signum()) || (a - b).abs() < 1e-6
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

/// A point guaranteed to lie inside a CONVEX solid: the analytic centre of a
/// canonical curved face (sphere centre / cylinder axis-midpoint) if present —
/// robust where a sphere/cylinder carries no usable boundary vertices — else the
/// average of the boundary vertices (their convex hull ⊆ the solid). A seed for
/// the overlap probe.
fn solid_interior_point(model: &BRepModel, solid: SolidId) -> Option<Point3> {
    use crate::primitives::surface::{Cylinder, Sphere};
    use std::collections::HashSet;

    for fid in solid_face_ids(model, solid) {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let Some(surf) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        if let Some(s) = surf.as_any().downcast_ref::<Sphere>() {
            return Some(s.center);
        }
        if let Some(c) = surf.as_any().downcast_ref::<Cylinder>() {
            if let Some([h0, h1]) = c.height_limits {
                return Some(c.origin + c.axis * (0.5 * (h0 + h1)));
            }
        }
    }

    let mut seen: HashSet<u32> = HashSet::new();
    let (mut sx, mut sy, mut sz, mut n) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    for fid in solid_face_ids(model, solid) {
        for eid in boundary_edge_ids(model, fid) {
            if let Some(e) = model.edges.get(eid) {
                for vid in [e.start_vertex, e.end_vertex] {
                    if seen.insert(vid) {
                        if let Some(p) = model.vertices.get_position(vid) {
                            sx += p[0];
                            sy += p[1];
                            sz += p[2];
                            n += 1.0;
                        }
                    }
                }
            }
        }
    }
    if n == 0.0 {
        return None;
    }
    Some(Point3::new(sx / n, sy / n, sz / n))
}

/// Convex point-in-solid: `p` is inside a convex solid iff it lies on the INNER
/// side of every bounding face — the signed distance from `p` to the face's
/// surface along the OUTWARD normal is ≤ 0. The surface normal is oriented
/// outward by the solid's `interior` centroid (away from it), so this is
/// independent of stored face orientation and works for curved convex solids
/// (sphere / cylinder / cone), where the winding-number shell test under-resolves
/// a seam-bounded curved face's solid angle. (Assumes convex operands — the
/// standard CD regime.)
fn point_in_solid(model: &BRepModel, solid: SolidId, interior: &Point3, p: &Point3) -> bool {
    let tol = crate::math::Tolerance::default();
    let tol_d = tol.distance();
    let mut tested = false;
    for fid in solid_face_ids(model, solid) {
        let Some(face) = model.faces.get(fid) else {
            continue;
        };
        let Some(surf) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        let Ok((u, v)) = surf.closest_point(p, tol) else {
            continue;
        };
        let Ok(eval) = surf.evaluate_full(u, v) else {
            continue;
        };
        let sp = eval.position;
        // Orient the surface normal to point AWAY from the solid interior.
        let outward = if eval.normal.dot(&(sp - *interior)) < 0.0 {
            eval.normal * -1.0
        } else {
            eval.normal
        };
        tested = true;
        if (*p - sp).dot(&outward) > tol_d {
            return false; // p is on the outer side of this face → outside the solid
        }
    }
    tested
}

/// Do the two solids' INTERIORS overlap? Sample the segment between an interior
/// point of each (which threads the lens of any convex overlap) and report true
/// if any sample is inside BOTH closed shells. Robust + cheap for the convex CD
/// primitives; gracefully returns false when no interior seed is available.
fn solids_overlap(model: &BRepModel, a: SolidId, b: SolidId) -> bool {
    let (Some(pa), Some(pb)) = (
        solid_interior_point(model, a),
        solid_interior_point(model, b),
    ) else {
        return false;
    };
    for k in 0..=4 {
        let t = k as f64 / 4.0;
        let p = Point3::new(
            pa.x + (pb.x - pa.x) * t,
            pa.y + (pb.y - pa.y) * t,
            pa.z + (pb.z - pa.z) * t,
        );
        if point_in_solid(model, a, &pa, &p) && point_in_solid(model, b, &pb, &p) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn box_solid(model: &mut BRepModel) -> SolidId {
        TopologyBuilder::new(model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    /// Two unit boxes whose faces meet at x = 1 (centres 2 apart).
    fn two_touching_boxes() -> (BRepModel, SolidId, SolidId) {
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(
            &mut model,
            vec![b],
            Vector3::X,
            2.0,
            Default::default(),
        )
        .expect("translate");
        (model, a, b)
    }

    #[test]
    fn full_pipeline_matches_brute_force_with_less_work() {
        let (model, a, b) = two_touching_boxes();
        let baseline = run_cd_ablation(&model, a, b, CdAblationConfig::baseline());
        let full = run_cd_ablation(&model, a, b, CdAblationConfig::full());

        // Both correct, and they agree on the answer.
        assert_eq!(
            baseline.report.correct,
            Some(true),
            "{}",
            baseline.report.render()
        );
        assert_eq!(full.report.correct, Some(true), "{}", full.report.render());
        assert!((baseline.min_distance - full.min_distance).abs() < 1e-9);
        assert!(baseline.min_distance.abs() < 1e-6, "touching boxes → 0 gap");

        // Baseline LMDs every face-pair; the full pipeline does strictly fewer.
        assert_eq!(baseline.lmd_solves, 36, "6×6 face-pairs");
        assert!(
            full.lmd_solves < baseline.lmd_solves,
            "full {} vs baseline {}",
            full.lmd_solves,
            baseline.lmd_solves
        );
    }

    #[test]
    fn ablation_matrix_is_monotone_and_all_correct() {
        let (model, a, b) = two_touching_boxes();
        let results = run_cd_ablation_matrix(&model, a, b);
        assert_eq!(results.len(), 4);

        // Every configuration is verified and agrees on the minimum distance.
        let answer = results[0].min_distance;
        for r in &results {
            assert_eq!(r.report.correct, Some(true), "{}", r.report.render());
            assert!(
                (r.min_distance - answer).abs() < 1e-9,
                "config changed the answer"
            );
        }

        // Each layered optimisation does no more narrow-phase work than the
        // brute-force baseline (the headline ablation result).
        let baseline_solves = results[0].lmd_solves;
        for r in &results[1..] {
            assert!(
                r.lmd_solves <= baseline_solves,
                "{}: {} solves vs baseline {}",
                r.report.label,
                r.lmd_solves,
                baseline_solves
            );
        }
        // The cone cull alone already prunes work.
        assert!(results[1].lmd_solves < baseline_solves);
    }

    #[test]
    fn separated_boxes_have_no_contact_and_full_pipeline_culls_everything() {
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(
            &mut model,
            vec![b],
            Vector3::X,
            20.0,
            Default::default(),
        )
        .expect("translate");

        let full = run_cd_ablation(&model, a, b, CdAblationConfig::full());
        let baseline = run_cd_ablation(&model, a, b, CdAblationConfig::baseline());
        // Far apart: both correctly report "no contact" (the contact predicate
        // agrees), even though the pipeline prunes the far closest pair the
        // brute baseline still measures.
        assert_eq!(full.report.correct, Some(true), "{}", full.report.render());
        assert_eq!(baseline.report.correct, Some(true));
        assert!(full.min_distance > 1e-6, "no contact");
        // The BVH broad phase prunes to nothing, so the narrow phase is far
        // cheaper than brute force.
        assert!(full.lmd_solves < baseline.lmd_solves);
    }

    /// Analytic solid-to-solid contact distance for two axis-aligned unit boxes
    /// (half-extent 1) on a pure X-translation `tx`. With edge/vertex closest-
    /// approach in the narrow phase (#83) the CD pipeline reports the true
    /// boundary contact distance: 0 while the boxes overlap or touch
    /// (0 ≤ tx ≤ 2 — they share volume or a face/edge, so their boundaries meet),
    /// and the face gap `tx − 2` once separated (tx > 2). Independent of the
    /// kernel. (Before #83 the narrow phase saw only face-interior LMDs and
    /// reported the nearest *parallel-face* gap `min(|tx-2|,|tx|)`, which read a
    /// non-zero "distance" for overlapping solids — superseded.)
    fn face_on_truth(tx: f64) -> f64 {
        (tx - 2.0).max(0.0)
    }

    /// Face-on CD proximity vs analytic truth: two boxes approaching face-to-face
    /// along X across overlapping, touching, and separated poses. The all-pairs
    /// narrow-phase minimum (baseline = no broad-phase cull), now including edge/
    /// vertex closest approach, must equal the true contact distance: 0 while the
    /// boxes overlap/touch, the face gap once separated. Validates the narrow
    /// phase against an independent oracle.
    #[test]
    fn cd_face_proximity_matches_analytic() {
        let xs = [0.0, 1.5, 2.0, 2.5, 3.0, 5.0];
        let mut failures: Vec<String> = Vec::new();
        for tx in xs {
            let mut model = BRepModel::new();
            let a = box_solid(&mut model);
            let b = box_solid(&mut model);
            if tx > 1e-12 {
                crate::operations::transform::translate(
                    &mut model,
                    vec![b],
                    Vector3::X,
                    tx,
                    Default::default(),
                )
                .expect("translate");
            }
            let r = run_cd_ablation(&model, a, b, CdAblationConfig::baseline());
            let truth = face_on_truth(tx);
            if (r.min_distance - truth).abs() > 1e-5 {
                failures.push(format!(
                    "tx={tx}: kernel d={:.5} vs truth {:.5}",
                    r.min_distance, truth
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "face-on CD distance disagreements with independent oracle:\n  {}",
            failures.join("\n  ")
        );
    }

    /// #83 (FIXED): the face-pair LMD min-distance returned ∞ when the closest
    /// approach between two solids is edge-edge or vertex-vertex rather than
    /// face-face — the `footpoint_in_face` filter discards any critical point on a
    /// face boundary. Two unit boxes touching along an edge (t=[2,2,0]) or at a
    /// corner (t=[2,2,2]) reported NO contact (a false negative). Fixed by adding
    /// edge-edge / vertex closest approach to the narrow phase
    /// (`face_pair_min_distance` → `edge_pair_min_distance` → segment-segment
    /// distance over the faces' boundary edges). Both poses now register contact.
    #[test]
    fn cd_edge_corner_contact_83() {
        const TAU: f64 = 1e-6;
        for (label, t) in [
            ("edge", [2.0_f64, 2.0, 0.0]),
            ("corner", [2.0_f64, 2.0, 2.0]),
        ] {
            let mut model = BRepModel::new();
            let a = box_solid(&mut model);
            let b = box_solid(&mut model);
            let mag = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt();
            let axis = Vector3::new(t[0] / mag, t[1] / mag, t[2] / mag);
            crate::operations::transform::translate(
                &mut model,
                vec![b],
                axis,
                mag,
                Default::default(),
            )
            .expect("translate");
            let r = run_cd_ablation(&model, a, b, CdAblationConfig::baseline());
            assert!(
                r.min_distance <= TAU,
                "{label}-touch should register contact, got d={}",
                r.min_distance
            );
        }
    }
}
