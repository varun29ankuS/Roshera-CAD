//! Extrude correctness harness (GEOM-HARNESS).
//!
//! Invariant: extruding a closed planar profile of area `A` by distance `d`
//! yields a solid of volume `A·d`, and that solid is watertight. This pairs the
//! op-specific conservation law (volume = base area × height) with the universal
//! [`crate::harness::watertight::is_watertight`] check.

use crate::harness::watertight::is_watertight;
use crate::math::vector3::Point3;
use crate::operations::extrude::{extrude_profile, ExtrudeOptions};
use crate::primitives::curve::Line;
use crate::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use crate::primitives::topology_builder::BRepModel;

/// Result of an extrude invariant check.
#[derive(Debug, Clone)]
pub struct ExtrudeCheck {
    pub volume: Option<f64>,
    pub expected_volume: f64,
    /// `volume ≈ base_area · distance`.
    pub volume_ok: bool,
    /// The extruded solid is watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Extrude a `width × height` rectangle by `distance` and check that the result
/// has volume `width·height·distance` and is watertight.
pub fn extrude_box_invariants(
    width: f64,
    height: f64,
    distance: f64,
    rel_tol: f64,
) -> ExtrudeCheck {
    let mut model = BRepModel::new();
    let edges = make_rectangle(&mut model, width, height);
    let options = ExtrudeOptions {
        distance,
        ..ExtrudeOptions::default()
    };
    let expected_volume = width * height * distance;

    let (volume, volume_ok, watertight) = match extrude_profile(&mut model, edges, options) {
        Ok(solid) => {
            let volume = model.calculate_solid_volume(solid);
            let volume_ok = volume.is_some_and(|v| within_rel(v, expected_volume, rel_tol));
            // Extruded box is planar → exact mesh volume; a tight watertight tol.
            let watertight = is_watertight(&mut model, solid, 0.01, rel_tol.max(1e-3));
            (volume, volume_ok, watertight)
        }
        Err(_) => (None, false, false),
    };

    ExtrudeCheck {
        volume,
        expected_volume,
        volume_ok,
        watertight,
        all_hold: volume_ok && watertight,
    }
}

// ---------------------------------------------------------------------------
// helpers (private) — mirror operations/extrude.rs profile fixtures.
// ---------------------------------------------------------------------------

fn make_rectangle(model: &mut BRepModel, width: f64, height: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(width, 0.0, 0.0);
    let v2 = model.vertices.add(width, height, 0.0);
    let v3 = model.vertices.add(0.0, height, 0.0);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

fn add_line_edge(model: &mut BRepModel, v_start: u32, v_end: u32) -> EdgeId {
    let s = model.vertices.get(v_start).expect("start vertex").position;
    let e = model.vertices.get(v_end).expect("end vertex").position;
    let line = Line::new(Point3::from(s), Point3::from(e));
    let curve_id = model.curves.add(Box::new(line));
    let edge = Edge::new_auto_range(0, v_start, v_end, curve_id, EdgeOrientation::Forward);
    model.edges.add(edge)
}

fn within_rel(a: f64, b: f64, tol: f64) -> bool {
    let scale = a.abs().max(b.abs()).max(1.0);
    (a - b).abs() / scale <= tol
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extruded_box_volume_and_watertight() {
        let c = extrude_box_invariants(2.0, 3.0, 4.0, 1e-2);
        assert!(
            c.volume_ok,
            "volume {:?} vs {}",
            c.volume, c.expected_volume
        );
        assert!(c.watertight, "extruded box not watertight");
        assert!(c.all_hold);
        assert!((c.volume.unwrap() - 24.0).abs() < 0.1);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

        /// V(extrude) = base_area · distance, and the result is watertight, for
        /// any positive rectangle and height.
        #[test]
        fn pp_extrude_volume_is_area_times_height(
            w in 0.5f64..12.0,
            h in 0.5f64..12.0,
            d in 0.5f64..20.0,
        ) {
            let c = extrude_box_invariants(w, h, d, 2e-2);
            prop_assert!(c.volume_ok, "w={w} h={h} d={d}: {:?}", c);
            prop_assert!(c.watertight, "w={w} h={h} d={d} not watertight");
        }
    }
}
