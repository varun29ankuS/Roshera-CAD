//! Analytic ε-erosion of a solid's canonical surfaces (CD-φ.2.4).
//!
//! Eroding a solid by `ε` shrinks it inward — every boundary surface moves a
//! distance `ε` toward the interior. For the canonical surfaces this is *exact*:
//! a plane stays a plane (shifted), a cylinder/sphere loses `ε` of radius, a cone
//! keeps its half-angle with a shifted apex, a torus loses `ε` of tube radius
//! (Crozet, *Smooth-BRep CD*, Sec 1.3.1.2 — erosion makes sharp boundaries
//! smooth so contact footpoints don't get pinned to a crease).
//!
//! This module computes the eroded **surfaces** analytically via
//! [`Surface::offset`], with the inward sign taken from each face's orientation.
//! It does not re-stitch topology or round convex edges into `ε`-fillets — the
//! geometric edge-rounding step is a fillet operation (`operations::fillet`) over
//! the convex edges, layered on top of these offset surfaces.

use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::surface::Surface;
use crate::primitives::topology_builder::BRepModel;

/// The supporting surface of `face_id` offset **inward** (into the solid) by
/// `eps` — the exact erosion of that face's geometry. `None` if the face or
/// surface is missing. Works for every surface type; exact for canonical
/// surfaces, an offset surface for free-form.
pub fn erode_face_surface(
    model: &BRepModel,
    face_id: FaceId,
    eps: f64,
) -> Option<Box<dyn Surface>> {
    let face = model.faces.get(face_id)?;
    let surface = model.surfaces.get(face.surface_id)?;
    // `Surface::offset(d)` moves the surface a distance `d` along its own
    // normal. The face's *outward* normal is the surface normal times the
    // orientation sign, so moving inward by `eps` is `offset(-eps · sign)`.
    let distance = -eps * face.orientation.sign();
    Some(surface.offset(distance))
}

/// Every boundary face of `solid_id` paired with its inward-eroded surface — the
/// analytic ε-erosion of the solid's geometry (surfaces only; topology
/// unchanged).
pub fn erode_solid_surfaces(
    model: &BRepModel,
    solid_id: SolidId,
    eps: f64,
) -> Vec<(FaceId, Box<dyn Surface>)> {
    solid_face_ids(model, solid_id)
        .into_iter()
        .filter_map(|fid| erode_face_surface(model, fid, eps).map(|s| (fid, s)))
        .collect()
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
    use crate::math::vector3::Vector3;
    use crate::primitives::surface::{Plane, Sphere};
    use crate::primitives::topology_builder::TopologyBuilder;

    const X: Vector3 = Vector3::X;

    #[test]
    fn eroding_a_sphere_shrinks_its_radius() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_sphere_3d(Vector3::new(0.0, 0.0, 0.0), 2.0)
            .expect("sphere");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("solid");

        let eroded = erode_solid_surfaces(&model, solid, 0.3);
        assert!(!eroded.is_empty());
        for (_, surf) in &eroded {
            let sp = surf
                .as_any()
                .downcast_ref::<Sphere>()
                .expect("eroded sphere stays a sphere");
            assert!(
                (sp.radius - 1.7).abs() < 1e-9,
                "radius {} (want 2.0 − 0.3)",
                sp.radius
            );
            // Centre unchanged.
            assert!(sp.center.magnitude() < 1e-9);
        }
    }

    #[test]
    fn eroding_a_box_moves_each_face_inward() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("box");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("solid");

        // The +X face sits at x = 1 with outward normal +X; eroding by 0.25 must
        // shift its plane to x = 0.75 (toward the centre), normal unchanged.
        let plus_x = model
            .faces
            .iter()
            .find(|(_, f)| {
                model
                    .surfaces
                    .get(f.surface_id)
                    .and_then(|s| s.as_any().downcast_ref::<Plane>())
                    .map(|p| p.normal.dot(&X).abs() > 0.99 && p.origin.dot(&X) > 0.5)
                    .unwrap_or(false)
            })
            .map(|(id, _)| id)
            .expect("+X face");

        let surf = erode_face_surface(&model, plus_x, 0.25).expect("eroded");
        let pl = surf
            .as_any()
            .downcast_ref::<Plane>()
            .expect("eroded plane stays a plane");
        assert!(
            (pl.origin.dot(&X) - 0.75).abs() < 1e-9,
            "eroded +X plane at x={} (want 0.75)",
            pl.origin.dot(&X)
        );
        assert!(
            (pl.normal - X).magnitude() < 1e-9,
            "normal must be unchanged"
        );
    }

    #[test]
    fn erosion_is_monotone_in_epsilon() {
        let mut model = BRepModel::new();
        TopologyBuilder::new(&mut model)
            .create_sphere_3d(Vector3::new(0.0, 0.0, 0.0), 5.0)
            .expect("sphere");
        let solid = model.solids.iter().last().map(|(id, _)| id).expect("solid");

        let r = |eps: f64| -> f64 {
            erode_solid_surfaces(&model, solid, eps)[0]
                .1
                .as_any()
                .downcast_ref::<Sphere>()
                .expect("sphere")
                .radius
        };
        // More erosion → smaller radius, exactly linear.
        assert!((r(0.0) - 5.0).abs() < 1e-9);
        assert!((r(1.0) - 4.0).abs() < 1e-9);
        assert!((r(2.5) - 2.5).abs() < 1e-9);
    }
}
