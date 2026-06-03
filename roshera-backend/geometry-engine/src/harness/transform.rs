//! Transform correctness harness (GEOM-HARNESS).
//!
//! Invariant: a **rigid** transform (rotation + translation) is an isometry —
//! it preserves volume exactly and maps a watertight solid to a watertight
//! solid. The harness applies an arbitrary rotation-then-translation to a box
//! and checks both.

use crate::harness::watertight::is_watertight;
use crate::math::vector3::{Point3, Vector3};
use crate::operations::transform::{rotate, translate};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// Result of a rigid-transform invariant check.
#[derive(Debug, Clone)]
pub struct TransformCheck {
    pub volume_before: Option<f64>,
    pub volume_after: Option<f64>,
    /// Volume is unchanged by the rigid motion.
    pub volume_preserved: bool,
    /// The transformed solid is still watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Build a `size³` box, rotate it `angle` about `axis` (through the origin), then
/// translate it by `translation`, and check that the volume is preserved and the
/// result is watertight.
pub fn rigid_transform_invariants(
    size: f64,
    axis: Vector3,
    angle: f64,
    translation: Vector3,
    rel_tol: f64,
) -> TransformCheck {
    let mut model = BRepModel::new();
    let Some(solid) = make_box(&mut model, size) else {
        return failed();
    };
    let volume_before = model.calculate_solid_volume(solid);

    let axis = axis.normalize().unwrap_or(Vector3::Z);
    if rotate(
        &mut model,
        vec![solid],
        Point3::ZERO,
        axis,
        angle,
        Default::default(),
    )
    .is_err()
    {
        return failed();
    }
    let dist = translation.magnitude();
    if dist > 1e-9 {
        let dir = translation.normalize().unwrap_or(Vector3::X);
        if translate(&mut model, vec![solid], dir, dist, Default::default()).is_err() {
            return failed();
        }
    }

    let volume_after = model.calculate_solid_volume(solid);
    let volume_preserved = match (volume_before, volume_after) {
        (Some(a), Some(b)) => within_rel(a, b, rel_tol),
        _ => false,
    };
    let watertight = is_watertight(&mut model, solid, 0.01, rel_tol.max(1e-3));

    TransformCheck {
        volume_before,
        volume_after,
        volume_preserved,
        watertight,
        all_hold: volume_preserved && watertight,
    }
}

fn make_box(model: &mut BRepModel, size: f64) -> Option<SolidId> {
    TopologyBuilder::new(model)
        .create_box_3d(size, size, size)
        .ok()?;
    model.solids.iter().last().map(|(id, _)| id)
}

fn failed() -> TransformCheck {
    TransformCheck {
        volume_before: None,
        volume_after: None,
        volume_preserved: false,
        watertight: false,
        all_hold: false,
    }
}

fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rigid_motion_preserves_volume_and_watertightness() {
        let c = rigid_transform_invariants(
            2.0,
            Vector3::new(1.0, 1.0, 1.0),
            0.7,
            Vector3::new(3.0, 1.0, 2.0),
            1e-2,
        );
        assert!(c.volume_preserved, "{c:?}");
        assert!(c.watertight, "transformed box not watertight: {c:?}");
        assert!((c.volume_after.unwrap() - 8.0).abs() < 0.05, "{c:?}");
    }

    use proptest::prelude::*;

    fn unit_axis() -> impl Strategy<Value = Vector3> {
        (-1.0f64..1.0, -1.0f64..1.0, -1.0f64..1.0).prop_filter_map("nonzero", |(x, y, z)| {
            Vector3::new(x, y, z).normalize().ok()
        })
    }

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        /// Any rotation-then-translation preserves a box's volume and keeps it
        /// watertight.
        #[test]
        fn pp_rigid_transform_preserves_volume(
            axis in unit_axis(),
            angle in -3.0f64..3.0,
            tx in -10.0f64..10.0,
            ty in -10.0f64..10.0,
            tz in -10.0f64..10.0,
        ) {
            let c = rigid_transform_invariants(
                3.0,
                axis,
                angle,
                Vector3::new(tx, ty, tz),
                1e-2,
            );
            prop_assert!(c.volume_preserved, "{c:?}");
            prop_assert!(c.watertight, "not watertight: {c:?}");
        }
    }
}
