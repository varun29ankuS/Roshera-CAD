//! Boolean broad-phase ablation — the harness applied beyond CD (HARNESS-β).
//!
//! A boolean (union / intersection / difference) only has to intersect the
//! face-pairs whose bounding boxes overlap; the rest of the `n_a × n_b` raw
//! pairs are spatially disjoint and contribute nothing. This study measures that
//! broad-phase funnel — raw face-pairs → AABB-overlapping pairs — so the
//! culling's contribution is a number, exactly as [`crate::harness::cd`] does for
//! contact determination.
//!
//! The cull is *sound by construction*: a face's AABB contains the face, so two
//! faces that share any point necessarily have overlapping AABBs — the cull can
//! never drop a pair the boolean needs. The harness verifies this on a known
//! coincident pair rather than asserting it.

use crate::harness::{AblationReport, StageMetric};
use crate::math::bbox::BBox;
use crate::math::vector3::Vector3;
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Measure the boolean broad-phase between two solids: how many of the raw
/// face-pairs survive AABB-overlap culling (the pairs a boolean must actually
/// intersect). `retained_witness`, when `Some((fa, fb))`, is a face-pair known to
/// be coincident; the report is marked verified iff that pair survives the cull
/// (soundness — the broad phase never drops a real intersection).
pub fn boolean_broad_phase_ablation(
    model: &BRepModel,
    solid_a: SolidId,
    solid_b: SolidId,
    retained_witness: Option<(FaceId, FaceId)>,
) -> AblationReport {
    let faces_a = solid_face_ids(model, solid_a);
    let faces_b = solid_face_ids(model, solid_b);
    let aabbs_a: Vec<Option<BBox>> = faces_a.iter().map(|&f| face_aabb(model, f)).collect();
    let aabbs_b: Vec<Option<BBox>> = faces_b.iter().map(|&f| face_aabb(model, f)).collect();

    let raw = faces_a.len() * faces_b.len();
    let mut overlapping = 0usize;
    let mut cost = 0u64;
    for ba in &aabbs_a {
        for bb in &aabbs_b {
            cost += 1;
            if let (Some(x), Some(y)) = (ba, bb) {
                if x.intersects(y) {
                    overlapping += 1;
                }
            }
        }
    }

    let mut report = AblationReport::new("boolean broad-phase (face-pair AABB cull)")
        .stage(StageMetric::new("raw_face_pairs", raw, raw, 0))
        .stage(StageMetric::new("aabb_overlap", raw, overlapping, cost));

    if let Some((fa, fb)) = retained_witness {
        let retained = match (face_aabb(model, fa), face_aabb(model, fb)) {
            (Some(x), Some(y)) => x.intersects(&y),
            _ => false,
        };
        report = report.verified(retained);
    }
    report
}

fn face_aabb(model: &BRepModel, face_id: FaceId) -> Option<BBox> {
    let face = model.faces.get(face_id)?;
    let mut pts = Vec::new();
    let mut loop_ids = vec![face.outer_loop];
    loop_ids.extend(face.inner_loops.iter().copied());
    for lid in loop_ids {
        if let Some(lp) = model.loops.get(lid) {
            for &eid in &lp.edges {
                if let Some(edge) = model.edges.get(eid) {
                    for vid in [edge.start_vertex, edge.end_vertex] {
                        if let Some(v) = model.vertices.get(vid) {
                            pts.push(Vector3::new(v.position[0], v.position[1], v.position[2]));
                        }
                    }
                }
            }
        }
    }
    BBox::from_points(&pts)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::surface::Plane;
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;

    fn box_solid(model: &mut BRepModel) -> SolidId {
        TopologyBuilder::new(model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    fn plane_face(model: &BRepModel, want_pos_x: bool) -> FaceId {
        model
            .faces
            .iter()
            .find(|(_, face)| {
                model
                    .surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .map(|p| {
                        p.normal.dot(&X).abs() > 0.99
                            && if want_pos_x {
                                p.origin.dot(&X) > 0.5
                            } else {
                                p.origin.dot(&X) < -0.5
                            }
                    })
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("axis face")
    }

    #[test]
    fn touching_boxes_broad_phase_culls_and_retains_the_contact() {
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(&mut model, vec![b], X, 2.0, Default::default())
            .expect("translate");

        // The coincident pair: A's +X face and B's −X face both at x = 1.
        let a_plus_x = plane_face(&model, true);
        // B's faces include a −X face now at x = 1 (B spans [1,3]); find it among
        // B's faces specifically by re-scanning for the one near x = 1.
        let b_minus_x = model
            .faces
            .iter()
            .filter(|(id, _)| *id != a_plus_x)
            .find(|(_, face)| {
                model
                    .surfaces
                    .get(face.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .map(|p| p.normal.dot(&X).abs() > 0.99 && (p.origin.dot(&X) - 1.0).abs() < 1e-6)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("B's face at x=1");

        let report = boolean_broad_phase_ablation(&model, a, b, Some((a_plus_x, b_minus_x)));
        let raw = report.stages[0].output;
        let overlap = report.stages[1].output;
        assert_eq!(raw, 36, "6×6 face-pairs");
        assert!(overlap < raw, "AABB culling must prune some pairs");
        assert!(overlap > 0, "the touching region is retained");
        assert_eq!(
            report.correct,
            Some(true),
            "coincident pair survives the cull"
        );
        assert!(report.render().contains("aabb_overlap"));
    }

    #[test]
    fn separated_boxes_broad_phase_culls_everything() {
        let mut model = BRepModel::new();
        let a = box_solid(&mut model);
        let b = box_solid(&mut model);
        crate::operations::transform::translate(&mut model, vec![b], X, 20.0, Default::default())
            .expect("translate");
        let report = boolean_broad_phase_ablation(&model, a, b, None);
        assert_eq!(report.stages[0].output, 36);
        assert_eq!(
            report.stages[1].output, 0,
            "far apart → no overlapping face-pairs"
        );
    }
}
