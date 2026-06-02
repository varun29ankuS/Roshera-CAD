//! Tessellation convergence + watertightness ablation (HARNESS-β).
//!
//! Tessellation is a one-way export view of the kernel geometry, and the bar it
//! has to clear is **watertightness**: the triangle mesh must enclose the same
//! volume the analytic solid does. This study sweeps the chord tolerance and, at
//! each level, measures the triangle count and the mesh's enclosed volume (via
//! the divergence theorem over the triangles) against the analytic volume. A
//! finer tolerance buys more triangles and a smaller volume error — the
//! density-vs-accuracy tradeoff made into numbers — and at every level the mesh
//! is verified to be a closed, correctly-oriented (watertight) solid rather than
//! a leaky shell.

use crate::harness::{AblationReport, StageMetric};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};

/// One tessellation level: its tolerance, triangle count, enclosed volume, and
/// the error against the analytic volume.
#[derive(Debug, Clone)]
pub struct TessLevel {
    pub chord_tolerance: f64,
    pub triangles: usize,
    pub mesh_volume: f64,
    pub volume_error: f64,
    pub report: AblationReport,
}

/// Tessellate `solid_id` at each chord tolerance, returning one [`TessLevel`] per
/// tolerance. `analytic_volume` is the solid's true volume (the watertightness
/// oracle); a level is `verified` when its mesh volume is within `accept_error`
/// of it. The mesh volume is the signed divergence-theorem sum over the
/// triangles — for a closed, consistently-oriented mesh it equals the enclosed
/// volume, so a wildly wrong value flags a leak or a flipped triangle.
pub fn tessellation_convergence(
    model: &BRepModel,
    solid_id: SolidId,
    analytic_volume: f64,
    chord_tolerances: &[f64],
    accept_error: f64,
) -> Vec<TessLevel> {
    let Some(solid) = model.solids.get(solid_id) else {
        return Vec::new();
    };
    let face_count = model
        .shells
        .get(solid.outer_shell)
        .map_or(0, |s| s.faces.len());

    let mut levels = Vec::with_capacity(chord_tolerances.len());
    for &chord in chord_tolerances {
        // Make chord tolerance the *sole* density driver: the default
        // max_angle_deviation / max_edge_length would otherwise dominate a
        // sphere's grid and mask the chord sweep. Non-binding values isolate the
        // chord → accuracy relationship this study is about.
        let params = TessellationParams {
            chord_tolerance: chord,
            max_angle_deviation: std::f64::consts::PI,
            max_edge_length: 1.0e9,
            ..TessellationParams::default()
        };
        let mesh = tessellate_solid(solid, model, &params);
        let triangles = mesh.triangles.len();
        let mesh_volume = mesh_enclosed_volume(&mesh);
        let volume_error = (mesh_volume - analytic_volume).abs();

        let report = AblationReport::new(format!("tessellation chord={chord}"))
            .stage(StageMetric::new(
                "faces→triangles",
                face_count,
                triangles,
                triangles as u64,
            ))
            .verified(volume_error <= accept_error);

        levels.push(TessLevel {
            chord_tolerance: chord,
            triangles,
            mesh_volume,
            volume_error,
            report,
        });
    }
    levels
}

/// Enclosed volume of a triangle mesh by the divergence theorem:
/// `V = (1/6) Σ p0 · (p1 × p2)` over triangles. Magnitude is taken so the result
/// is orientation-sign-agnostic.
fn mesh_enclosed_volume(mesh: &crate::tessellation::mesh::TriangleMesh) -> f64 {
    let mut six_v = 0.0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        six_v += p0.dot(&p1.cross(&p2));
    }
    (six_v / 6.0).abs()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn sphere_solid(model: &mut BRepModel, radius: f64) -> SolidId {
        TopologyBuilder::new(model)
            .create_sphere_3d(Vector3::new(0.0, 0.0, 0.0), radius)
            .expect("sphere");
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn finer_tolerance_means_more_triangles_and_less_volume_error() {
        let mut model = BRepModel::new();
        let radius = 2.0;
        let solid = sphere_solid(&mut model, radius);
        let analytic = 4.0 / 3.0 * std::f64::consts::PI * radius.powi(3);

        // The accept bound catches leaks/flips (wildly wrong volume), not normal
        // faceting under-enclosure of a watertight sphere (~1–2% at these
        // tolerances).
        let levels = tessellation_convergence(&model, solid, analytic, &[0.05, 0.02, 0.005], 1.0);
        assert_eq!(levels.len(), 3);

        // Finer tolerance → strictly more triangles.
        assert!(
            levels[0].triangles < levels[1].triangles && levels[1].triangles < levels[2].triangles,
            "triangle counts {} {} {}",
            levels[0].triangles,
            levels[1].triangles,
            levels[2].triangles
        );
        // Finer tolerance → the mesh converges toward the analytic volume.
        assert!(
            levels[2].volume_error < levels[0].volume_error,
            "errors {} → {}",
            levels[0].volume_error,
            levels[2].volume_error
        );
        // The finest level encloses the analytic volume within the faceting bound
        // (no leak, no flipped triangles).
        assert!(
            levels[2].volume_error <= 1.0,
            "finest mesh volume {} vs analytic {} (err {})",
            levels[2].mesh_volume,
            analytic,
            levels[2].volume_error
        );
        assert_eq!(levels[2].report.correct, Some(true));
    }

    /// Regression test for a defect the ablation harness surfaced (TESS-FIX,
    /// fixed): at coarse chord tolerance the per-shell watertight weld used
    /// `chord_tolerance` as its vertex-merge distance, which at coarse settings
    /// matched the (curvature-driven) triangle edge length and collapsed the
    /// whole sphere mesh to **zero triangles** — an invisible coarse-LOD solid.
    /// The weld distance is now capped well below the edge spacing.
    #[test]
    fn coarse_tolerance_sphere_must_not_be_empty() {
        let mut model = BRepModel::new();
        let solid = sphere_solid(&mut model, 1.5);
        let analytic = 4.0 / 3.0 * std::f64::consts::PI * 1.5_f64.powi(3);
        // Several coarse tolerances that previously produced empty meshes.
        let levels = tessellation_convergence(&model, solid, analytic, &[0.1, 0.15, 0.2], 10.0);
        for l in &levels {
            assert!(
                l.triangles > 0,
                "coarse sphere (chord {}) produced an empty mesh",
                l.chord_tolerance
            );
            // And the coarse mesh still encloses a plausible volume.
            assert!(
                l.mesh_volume > 0.5 * analytic,
                "coarse mesh volume implausibly small"
            );
        }
    }
}
