//! Sweep correctness harness (GEOM-HARNESS).
//!
//! Invariant: sweeping a planar profile of area `A` along a straight path
//! perpendicular to it, of length `L`, yields a prism of volume `A·L`, watertight.
//! (A straight perpendicular sweep is the canonical, exactly-checkable case; the
//! general curved sweep obeys a path-integral generalisation.)

use crate::harness::watertight::{is_watertight, mesh_volume};
use crate::math::vector3::Point3;
use crate::operations::{sweep_profile, CommonOptions, SweepOptions};
use crate::primitives::curve::Line;
use crate::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use crate::primitives::topology_builder::BRepModel;

/// Result of a sweep invariant check.
#[derive(Debug, Clone)]
pub struct SweepCheck {
    pub mesh_volume: Option<f64>,
    pub expected_volume: f64,
    /// Volume ≈ profile area × path length.
    pub prism_ok: bool,
    /// The swept solid is watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Sweep a `w×h` rectangle along +Z by `length` and check the prism volume +
/// watertightness.
pub fn sweep_prism_invariants(w: f64, h: f64, length: f64, rel_tol: f64) -> SweepCheck {
    let mut model = BRepModel::new();
    let profile = rectangle_xy(&mut model, w, h);
    let va = model.vertices.add(0.0, 0.0, 0.0);
    let vb = model.vertices.add(0.0, 0.0, length);
    let path = add_line_edge(&mut model, va, vb);
    let options = SweepOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    };
    let expected_volume = w * h * length;

    match sweep_profile(&mut model, profile, path, options) {
        Ok(solid) => {
            let mesh_volume = mesh_volume(&model, solid, 0.01);
            let prism_ok = mesh_volume.is_some_and(|m| within_rel(m, expected_volume, rel_tol));
            let watertight = is_watertight(&mut model, solid, 0.01, rel_tol.max(1e-3));
            SweepCheck {
                mesh_volume,
                expected_volume,
                prism_ok,
                watertight,
                all_hold: prism_ok && watertight,
            }
        }
        Err(_) => SweepCheck {
            mesh_volume: None,
            expected_volume,
            prism_ok: false,
            watertight: false,
            all_hold: false,
        },
    }
}

// ---------------------------------------------------------------------------
// helpers (private)
// ---------------------------------------------------------------------------

fn rectangle_xy(model: &mut BRepModel, w: f64, h: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(w, 0.0, 0.0);
    let v2 = model.vertices.add(w, h, 0.0);
    let v3 = model.vertices.add(0.0, h, 0.0);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

fn add_line_edge(model: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let s = model.vertices.get(a).expect("v").position;
    let e = model.vertices.get(b).expect("v").position;
    let curve_id = model
        .curves
        .add(Box::new(Line::new(Point3::from(s), Point3::from(e))));
    model.edges.add(Edge::new_auto_range(
        0,
        a,
        b,
        curve_id,
        EdgeOrientation::Forward,
    ))
}

fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swept_prism_volume_and_watertight() {
        // 2×3 rectangle swept 5 along +Z → 30.
        let c = sweep_prism_invariants(2.0, 3.0, 5.0, 1e-2);
        assert!(c.prism_ok, "{c:?}");
        assert!(c.watertight, "swept prism not watertight: {c:?}");
        assert!((c.expected_volume - 30.0).abs() < 1e-9);
    }

    use proptest::prelude::*;

    proptest! {
        // Two tessellations per case → keep the count modest for CI speed.
        #![proptest_config(ProptestConfig { cases: 12, ..ProptestConfig::default() })]

        /// V(sweep) = profile_area · path_length, watertight, for any rectangle
        /// and straight path.
        #[test]
        fn pp_sweep_prism_volume(
            w in 0.5f64..10.0,
            h in 0.5f64..10.0,
            l in 0.5f64..15.0,
        ) {
            let c = sweep_prism_invariants(w, h, l, 2e-2);
            prop_assert!(c.prism_ok, "w={w} h={h} l={l}: {c:?}");
            prop_assert!(c.watertight, "not watertight: {c:?}");
        }
    }
}
