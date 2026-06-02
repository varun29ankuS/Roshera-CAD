//! Fillet carrier-dispatch ablation — the harness on the blend pipeline (HARNESS-β).
//!
//! The constant-radius fillet pipeline (F4-α) chooses a *carrier surface* per
//! edge: an exact analytic blend — `CylindricalFillet` (plane∥plane straight
//! edge), `ToroidalFillet` (plane∥cylinder), `SphericalFillet` — where the
//! geometry permits, or a general `VariableRadiusFillet` / NURBS blend otherwise.
//! Analytic carriers are the cheap, exact path; the general carrier is the
//! expensive fallback. This study measures **analytic-carrier coverage**: fillet
//! a set of edges and report how many blend faces landed on the fast path versus
//! the fallback, verified against the operation succeeding.

use crate::harness::{AblationReport, StageMetric};
use crate::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use crate::primitives::edge::EdgeId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Result of a fillet carrier-dispatch ablation.
#[derive(Debug, Clone)]
pub struct FilletAblation {
    pub report: AblationReport,
    /// Blend faces created by the fillet.
    pub blend_faces: usize,
    /// Blend faces on an exact analytic carrier (the F4-α fast path).
    pub analytic_carriers: usize,
    /// Blend faces on the general (variable-radius / NURBS) carrier.
    pub general_carriers: usize,
    /// Whether the fillet operation succeeded.
    pub succeeded: bool,
}

/// Fillet `edges` of `solid` at constant `radius`, then classify each resulting
/// blend face by its carrier surface (analytic fast-path vs general fallback).
/// Mutates the model (the fillet is applied).
pub fn fillet_carrier_ablation(
    model: &mut BRepModel,
    solid: SolidId,
    edges: Vec<EdgeId>,
    radius: f64,
) -> FilletAblation {
    let edge_count = edges.len();
    let options = FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        ..Default::default()
    };

    let result = fillet_edges(model, solid, edges, options);
    let (blend_faces, analytic, general, succeeded) = match &result {
        Ok(faces) => {
            let mut analytic = 0usize;
            let mut general = 0usize;
            for &fid in faces {
                if let Some(face) = model.faces.get(fid) {
                    if let Some(surface) = model.surfaces.get(face.surface_id) {
                        if is_analytic_carrier(surface.type_name()) {
                            analytic += 1;
                        } else {
                            general += 1;
                        }
                    }
                }
            }
            (faces.len(), analytic, general, true)
        }
        Err(_) => (0, 0, 0, false),
    };

    let report = AblationReport::new(format!("fillet carrier dispatch r={radius}"))
        .stage(StageMetric::new(
            "edges→blend_faces",
            edge_count,
            blend_faces,
            blend_faces as u64,
        ))
        .stage(StageMetric::new(
            "analytic_carrier",
            blend_faces,
            analytic,
            0,
        ))
        .verified(succeeded);

    FilletAblation {
        report,
        blend_faces,
        analytic_carriers: analytic,
        general_carriers: general,
        succeeded,
    }
}

/// Is a blend surface an exact analytic carrier (vs the general fallback)?
fn is_analytic_carrier(type_name: &str) -> bool {
    matches!(
        type_name,
        "CylindricalFillet" | "ToroidalFillet" | "SphericalFillet"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn box_solid(model: &mut BRepModel) -> SolidId {
        TopologyBuilder::new(model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn straight_constant_edge_takes_the_analytic_carrier() {
        let mut model = BRepModel::new();
        let solid = box_solid(&mut model);
        // Any of the box's 12 straight edges; a constant-radius fillet of a
        // plane∥plane edge is exactly cylindrical → the analytic fast path.
        let edge = model.edges.iter().next().map(|(id, _)| id).expect("edge");

        let abl = fillet_carrier_ablation(&mut model, solid, vec![edge], 0.3);
        assert!(abl.succeeded, "fillet failed: {}", abl.report.render());
        assert!(abl.blend_faces > 0, "no blend faces produced");
        assert_eq!(
            abl.general_carriers, 0,
            "a plane∥plane constant-radius edge must not need the general carrier"
        );
        assert_eq!(
            abl.analytic_carriers, abl.blend_faces,
            "every blend face should be an analytic carrier"
        );
        assert_eq!(abl.report.correct, Some(true));
        assert!(abl.report.render().contains("analytic_carrier"));
    }
}
