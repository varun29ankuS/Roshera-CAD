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

use crate::harness::watertight::{is_watertight, mesh_volume};
use crate::harness::{AblationReport, StageMetric};
use crate::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use crate::primitives::edge::EdgeId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

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

// ---------------------------------------------------------------------------
// Fillet VOLUME invariant (GEOM-HARNESS)
// ---------------------------------------------------------------------------

/// Result of a fillet volume invariant check.
#[derive(Debug, Clone)]
pub struct FilletVolumeCheck {
    pub mesh_volume: Option<f64>,
    pub expected_volume: f64,
    /// Material removed by the round-over: `(1 − π/4)·r²·L`.
    pub removed: f64,
    pub edge_length: f64,
    /// Mesh volume matches `V_box − removed` to a fraction of `removed`.
    pub volume_ok: bool,
    /// The filleted solid is watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Fillet one straight edge of a `side³` box at constant `radius` and check the
/// removed-material volume + watertightness.
///
/// A constant-radius fillet of a 90° box edge replaces the sharp corner with a
/// quarter-cylinder of radius `r`: the removed cross-section is a square `r×r`
/// minus a quarter-disc, `r²(1 − π/4)`, uniform over the edge length `L`. So the
/// filleted solid has volume `V_box − (1 − π/4)·r²·L`. The match is asserted to a
/// fraction (`band_frac`) of the removed amount, so the oracle genuinely pins the
/// round-over volume rather than being swallowed by the box volume.
pub fn fillet_box_edge_volume_invariants(
    side: f64,
    radius: f64,
    band_frac: f64,
) -> FilletVolumeCheck {
    use std::f64::consts::PI;

    let mut model = BRepModel::new();
    let solid = match make_cube(&mut model, side) {
        Some(s) => s,
        None => return failed_volume(),
    };
    let box_volume = side * side * side;

    let Some(edge_id) = model.edges.iter().map(|(id, _)| id).next() else {
        return failed_volume();
    };
    let edge_length = match edge_len(&model, edge_id) {
        Some(l) => l,
        None => return failed_volume(),
    };

    let removed = (1.0 - PI / 4.0) * radius * radius * edge_length;
    let expected_volume = box_volume - removed;

    let options = FilletOptions {
        fillet_type: FilletType::Constant(radius),
        radius,
        ..Default::default()
    };
    if fillet_edges(&mut model, solid, vec![edge_id], options).is_err() {
        return FilletVolumeCheck {
            mesh_volume: None,
            expected_volume,
            removed,
            edge_length,
            volume_ok: false,
            watertight: false,
            all_hold: false,
        };
    }

    let mesh_volume = mesh_volume(&model, solid, 0.005);
    // Match the filleted volume to within `band_frac` of the removed amount —
    // an unfilleted (or wrong-radius) solid lands outside the band.
    let band = (band_frac * removed).max(1e-6);
    let volume_ok = mesh_volume.is_some_and(|m| (m - expected_volume).abs() < band);
    let watertight = is_watertight(&mut model, solid, 0.005, 5e-3);

    FilletVolumeCheck {
        mesh_volume,
        expected_volume,
        removed,
        edge_length,
        volume_ok,
        watertight,
        all_hold: volume_ok && watertight,
    }
}

fn make_cube(model: &mut BRepModel, side: f64) -> Option<SolidId> {
    TopologyBuilder::new(model)
        .create_box_3d(side, side, side)
        .ok()?;
    model.solids.iter().last().map(|(id, _)| id)
}

fn edge_len(model: &BRepModel, edge_id: EdgeId) -> Option<f64> {
    let edge = model.edges.get(edge_id)?;
    let a = model.vertices.get(edge.start_vertex)?.position;
    let b = model.vertices.get(edge.end_vertex)?.position;
    Some(((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt())
}

fn failed_volume() -> FilletVolumeCheck {
    FilletVolumeCheck {
        mesh_volume: None,
        expected_volume: 0.0,
        removed: 0.0,
        edge_length: 0.0,
        volume_ok: false,
        watertight: false,
        all_hold: false,
    }
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

    #[test]
    fn filleted_box_edge_removes_the_round_over_and_stays_watertight() {
        // 3³ box, edge length 3, r=1 → removed = (1−π/4)·1·3 ≈ 0.644;
        // V ≈ 26.36. Band = 30% of removed (≈0.19) — an unfilleted 27 is well
        // outside, so the oracle truly pins the round-over volume.
        let c = fillet_box_edge_volume_invariants(3.0, 1.0, 0.30);
        assert!(c.volume_ok, "{c:?}");
        assert!(c.watertight, "filleted box not watertight: {c:?}");
        assert!(
            c.removed > 0.6 && c.removed < 0.7,
            "removed = {}",
            c.removed
        );
    }

    use proptest::prelude::*;

    proptest! {
        // One fillet + two tessellations per case → modest count for CI speed.
        #![proptest_config(ProptestConfig { cases: 10, ..ProptestConfig::default() })]

        /// V(fillet) = V(box) − (1−π/4)·r²·L, watertight, for a range of cube
        /// sizes and radii (radius kept well below the half-face so the round-
        /// over does not consume an adjacent face).
        #[test]
        fn pp_fillet_round_over_volume(
            side in 3.0f64..8.0,
            r in 0.5f64..1.4,
        ) {
            let c = fillet_box_edge_volume_invariants(side, r, 0.35);
            prop_assert!(c.volume_ok, "side={side} r={r}: {c:?}");
            prop_assert!(c.watertight, "not watertight: {c:?}");
        }
    }
}
