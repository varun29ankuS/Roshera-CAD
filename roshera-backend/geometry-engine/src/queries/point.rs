//! Point spatial queries (#14 slice 1) — classify + nearest.
//!
//! The composable spatial-query core's point primitive, built on the analytic
//! ray-cast: `classify_point` answers inside / outside / on, and
//! `nearest_on_solid` returns the closest boundary point with the face it lies
//! on (recoverable). Sound: classification is exact ray-parity along a generic
//! direction over analytic surface crossings; "on" is exact surface distance —
//! never a tessellation lookup. A point inside a through-hole reads OUTSIDE the
//! solid, because the hole is genuinely not material.

use super::raycast::raycast_all;
use crate::math::{Point3, Vector3};
use crate::primitives::face::FaceId;
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::BRepModel;

/// Where a point sits relative to a solid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointClass {
    Inside,
    Outside,
    /// Within `tol` of the boundary surface.
    On,
}

/// Closest point on the solid's boundary to `p`, as `(face_id, surface_point,
/// distance)`. Slice 1 uses each face's surface closest-point clipped to its
/// trim; a query point whose true nearest is a trim EDGE (not a face interior)
/// is a refinement.
pub fn nearest_on_solid(
    model: &BRepModel,
    solid_id: SolidId,
    p: Point3,
) -> Option<(FaceId, Point3, f64)> {
    let solid = model.solids.get(solid_id)?;
    let mut shells = vec![solid.outer_shell];
    shells.extend_from_slice(&solid.inner_shells);

    let mut best: Option<(FaceId, Point3, f64)> = None;
    for shell_id in shells {
        let shell = match model.shells.get(shell_id) {
            Some(s) => s,
            None => continue,
        };
        for &face_id in &shell.faces {
            let face = match model.faces.get(face_id) {
                Some(f) => f,
                None => continue,
            };
            let surface = match model.surfaces.get(face.surface_id) {
                Some(s) => s,
                None => continue,
            };
            let (u, v) = match surface.closest_point(&p, model.tolerance()) {
                Ok(uv) => uv,
                Err(_) => continue,
            };
            if !crate::tessellation::surface::point_inside_face_uv(u, v, face, model) {
                continue;
            }
            let sp = match surface.point_at(u, v) {
                Ok(sp) => sp,
                Err(_) => continue,
            };
            let d = (p - sp).magnitude();
            if best.map(|b| d < b.2).unwrap_or(true) {
                best = Some((face_id, sp, d));
            }
        }
    }
    best
}

/// Classify `p`: `On` if within `tol` of the boundary, else `Inside`/`Outside`
/// by exact ray-parity (odd crossings = inside). The ray direction is a generic
/// irrational-ish vector so it never grazes an axis-aligned face/seam (those are
/// measure-zero), keeping the crossing count exact.
pub fn classify_point(model: &BRepModel, solid_id: SolidId, p: Point3, tol: f64) -> PointClass {
    if let Some((_, _, d)) = nearest_on_solid(model, solid_id, p) {
        if d <= tol {
            return PointClass::On;
        }
    }
    let dir = Vector3::new(0.5212, 0.3389, 0.7831);
    let crossings = raycast_all(model, solid_id, p, dir).len();
    if crossings % 2 == 1 {
        PointClass::Inside
    } else {
        PointClass::Outside
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::math::Vector3;
    use crate::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};

    fn sid(g: GeometryId) -> SolidId {
        match g {
            GeometryId::Solid(s) => s,
            o => panic!("expected solid, got {o:?}"),
        }
    }

    #[test]
    fn box_classify_inside_outside_on() {
        // Box 20³ centred at origin (each half-extent 10).
        let mut m = BRepModel::new();
        let b = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("box"));
        assert_eq!(
            classify_point(&m, b, Point3::ZERO, 1e-6),
            PointClass::Inside
        );
        assert_eq!(
            classify_point(&m, b, Point3::new(50.0, 0.0, 0.0), 1e-6),
            PointClass::Outside
        );
        // A point exactly on the +Z face.
        assert_eq!(
            classify_point(&m, b, Point3::new(0.0, 0.0, 10.0), 1e-5),
            PointClass::On
        );
        // nearest: a point above the top is nearest the +Z face at distance 5.
        let (_, sp, d) = nearest_on_solid(&m, b, Point3::new(0.0, 0.0, 15.0)).expect("nearest");
        assert!((d - 5.0).abs() < 1e-6, "distance {d} != 5");
        assert!((sp.z - 10.0).abs() < 1e-6, "nearest on top face z=10");
    }

    #[test]
    fn cylinder_and_sphere_classify() {
        let mut m = BRepModel::new();
        let c = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
            .expect("cyl"));
        assert_eq!(
            classify_point(&m, c, Point3::new(0.0, 0.0, 15.0), 1e-6),
            PointClass::Inside
        );
        assert_eq!(
            classify_point(&m, c, Point3::new(20.0, 0.0, 15.0), 1e-6),
            PointClass::Outside
        );

        let mut m2 = BRepModel::new();
        let s = sid(TopologyBuilder::new(&mut m2)
            .create_sphere_3d(Point3::ZERO, 12.0)
            .expect("sphere"));
        assert_eq!(
            classify_point(&m2, s, Point3::ZERO, 1e-6),
            PointClass::Inside
        );
        assert_eq!(
            classify_point(&m2, s, Point3::new(0.0, 0.0, 12.0), 1e-5),
            PointClass::On
        );
        assert_eq!(
            classify_point(&m2, s, Point3::new(0.0, 0.0, 20.0), 1e-6),
            PointClass::Outside
        );
    }

    #[test]
    fn point_in_through_hole_is_outside_the_solid() {
        // A plate with a Ø20 through-bore. A point on the bore AXIS is inside
        // the HOLE, i.e. OUTSIDE the material — the soundness check: the query
        // sees the hole as genuinely not part of the solid.
        let mut m = BRepModel::new();
        let plate = sid(TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 50.0, 16.0)
            .expect("plate"));
        let bore = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
            .expect("bore"));
        let part = boolean_operation(
            &mut m,
            plate,
            bore,
            BooleanOp::Difference,
            BooleanOptions::default(),
        )
        .expect("bore");
        // On the bore axis, mid-plate → inside the hole → OUTSIDE the solid.
        assert_eq!(
            classify_point(&m, part, Point3::ZERO, 1e-6),
            PointClass::Outside,
            "a point in the through-hole is outside the material"
        );
        // In the material (between bore wall r=10 and plate edge) → Inside.
        assert_eq!(
            classify_point(&m, part, Point3::new(20.0, 0.0, 0.0), 1e-6),
            PointClass::Inside,
            "a point in the plate material is inside"
        );
    }
}
