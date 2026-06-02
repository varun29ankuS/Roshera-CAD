//! Universal watertightness oracle — the one correctness check every geometry
//! operation's output must pass.
//!
//! A solid is *watertight* when its boundary is a closed, consistently-oriented
//! surface enclosing a well-defined volume. The kernel can assert this cheaply
//! and universally: tessellate the solid and compare the mesh's enclosed volume
//! (the divergence-theorem sum over the triangles) against the analytic
//! mass-properties volume. A leak (open seam) or a flipped triangle makes the
//! divergence sum diverge wildly from the true volume, so agreement within the
//! faceting tolerance certifies the boundary is closed.
//!
//! Every op harness in this module — boolean, fillet, extrude, revolve, … — can
//! call [`is_watertight`] on its result; it is the shared, operation-agnostic
//! correctness primitive the whole geometry module is held to.

use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;
use crate::tessellation::{tessellate_solid, TessellationParams};

/// The analytic (mass-properties) volume of a solid, or `None` if it can't be
/// computed.
pub fn analytic_volume(model: &mut BRepModel, solid: SolidId) -> Option<f64> {
    model.calculate_solid_volume(solid)
}

/// The volume enclosed by the solid's tessellated mesh at chord tolerance
/// `chord`, via the divergence theorem `V = (1/6) Σ p0·(p1×p2)`. `None` if the
/// solid is missing or tessellates to nothing.
pub fn mesh_volume(model: &BRepModel, solid: SolidId, chord: f64) -> Option<f64> {
    let solid_ref = model.solids.get(solid)?;
    let params = TessellationParams {
        chord_tolerance: chord,
        ..TessellationParams::default()
    };
    let mesh = tessellate_solid(solid_ref, model, &params);
    if mesh.triangles.is_empty() {
        return None;
    }
    let mut six_v = 0.0;
    for tri in &mesh.triangles {
        let p0 = mesh.vertices[tri[0] as usize].position;
        let p1 = mesh.vertices[tri[1] as usize].position;
        let p2 = mesh.vertices[tri[2] as usize].position;
        six_v += p0.dot(&p1.cross(&p2));
    }
    Some((six_v / 6.0).abs())
}

/// Is `solid` watertight? Its tessellated mesh must enclose the analytic volume
/// within the relative tolerance `rel_tol` (a few percent absorbs faceting; a
/// leak or flip produces a far larger discrepancy). `false` if either volume is
/// uncomputable (which is itself a failure).
pub fn is_watertight(model: &mut BRepModel, solid: SolidId, chord: f64, rel_tol: f64) -> bool {
    let Some(analytic) = model.calculate_solid_volume(solid) else {
        return false;
    };
    let Some(mesh) = mesh_volume(model, solid, chord) else {
        return false;
    };
    let scale = analytic.abs().max(mesh.abs()).max(1.0);
    (analytic - mesh).abs() / scale <= rel_tol
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::vector3::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::operations::transform::translate;
    use crate::primitives::topology_builder::TopologyBuilder;

    fn last_solid(model: &BRepModel) -> SolidId {
        model.solids.iter().last().map(|(id, _)| id).expect("solid")
    }

    #[test]
    fn primitives_are_watertight() {
        // Box (exact), sphere and cylinder (curved, faceted) must all enclose
        // their analytic volume within the faceting tolerance.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let box_solid = last_solid(&model);
        assert!(
            is_watertight(&mut model, box_solid, 0.01, 1e-6),
            "box leaks"
        );

        let mut m2 = BRepModel::new();
        TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Vector3::new(0.0, 0.0, 0.0), 3.0)
            .expect("sphere");
        let sphere = last_solid(&m2);
        assert!(is_watertight(&mut m2, sphere, 0.01, 0.03), "sphere leaks");

        let mut m3 = BRepModel::new();
        TopologyBuilder::new(&mut m3)
            .create_cylinder_3d(Vector3::new(0.0, 0.0, 0.0), Vector3::Z, 2.0, 5.0)
            .expect("cylinder");
        let cyl = last_solid(&m3);
        assert!(is_watertight(&mut m3, cyl, 0.01, 0.03), "cylinder leaks");
    }

    #[test]
    fn boolean_result_is_watertight() {
        // A union of two overlapping boxes must itself be a closed solid.
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("a");
        let a = last_solid(&model);
        TopologyBuilder::new(&mut model)
            .create_box_3d(4.0, 4.0, 4.0)
            .expect("b");
        let b = last_solid(&model);
        translate(&mut model, vec![b], Vector3::X, 2.0, Default::default()).expect("translate");

        let result = boolean_operation(
            &mut model,
            a,
            b,
            BooleanOp::Union,
            BooleanOptions::default(),
        )
        .expect("union");
        assert!(
            is_watertight(&mut model, result, 0.01, 1e-3),
            "boolean union result is not watertight"
        );
    }

    #[test]
    fn mesh_volume_matches_analytic_for_a_box() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 3.0, 4.0)
            .expect("box");
        let solid = last_solid(&model);
        let analytic = analytic_volume(&mut model, solid).expect("analytic");
        let mesh = mesh_volume(&model, solid, 0.01).expect("mesh");
        assert!((analytic - 24.0).abs() < 1e-6, "analytic {analytic}");
        assert!((mesh - 24.0).abs() < 1e-6, "mesh {mesh}");
    }
}
