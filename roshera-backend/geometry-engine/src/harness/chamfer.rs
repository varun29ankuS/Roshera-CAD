//! Chamfer correctness harness (GEOM-HARNESS).
//!
//! Invariant: an equal-distance (45°) chamfer of setback `d` on a single
//! straight convex edge removes a triangular-prism wedge of volume `½·d²·L`,
//! where `L` is the edge length. So the chamfered solid has volume
//! `V_box − ½·d²·L`, and it stays watertight. The harness pairs that wedge oracle
//! with the universal [`crate::harness::watertight::is_watertight`] check.

use crate::harness::watertight::{is_watertight, mesh_volume};
use crate::operations::chamfer::{ChamferType, PropagationMode};
use crate::operations::{chamfer_edges, ChamferOptions, CommonOptions};
use crate::primitives::solid::SolidId;
use crate::primitives::topology_builder::{BRepModel, TopologyBuilder};

/// Result of a chamfer invariant check.
#[derive(Debug, Clone)]
pub struct ChamferCheck {
    pub mesh_volume: Option<f64>,
    pub expected_volume: f64,
    pub edge_length: f64,
    /// Volume = box volume − wedge (`½·d²·L`).
    pub wedge_ok: bool,
    /// The chamfered solid is watertight.
    pub watertight: bool,
    pub all_hold: bool,
}

/// Chamfer the first edge of a `w×h×d` box by equal setback `dist` and check the
/// removed-wedge volume + watertightness.
pub fn chamfer_box_edge_invariants(
    w: f64,
    h: f64,
    d: f64,
    dist: f64,
    rel_tol: f64,
) -> ChamferCheck {
    let mut model = BRepModel::new();
    let solid = match make_box(&mut model, w, h, d) {
        Some(s) => s,
        None => return failed(),
    };
    let box_volume = w * h * d;

    let Some(edge_id) = model.edges.iter().map(|(id, _)| id).next() else {
        return failed();
    };
    let edge_length = match model.edges.get(edge_id) {
        Some(edge) => {
            let p0 = model.vertices.get(edge.start_vertex).map(|v| v.position);
            let p1 = model.vertices.get(edge.end_vertex).map(|v| v.position);
            match (p0, p1) {
                (Some(a), Some(b)) => {
                    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
                }
                _ => return failed(),
            }
        }
        None => return failed(),
    };

    let options = ChamferOptions {
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        chamfer_type: ChamferType::EqualDistance(dist),
        distance1: dist,
        distance2: dist,
        symmetric: true,
        propagation: PropagationMode::None,
        ..Default::default()
    };

    let expected_volume = box_volume - 0.5 * dist * dist * edge_length;

    if chamfer_edges(&mut model, solid, vec![edge_id], options).is_err() {
        return ChamferCheck {
            mesh_volume: None,
            expected_volume,
            edge_length,
            wedge_ok: false,
            watertight: false,
            all_hold: false,
        };
    }

    let mesh_volume = mesh_volume(&model, solid, 0.01);
    let wedge_ok = mesh_volume.is_some_and(|m| within_rel(m, expected_volume, rel_tol));
    let watertight = is_watertight(&mut model, solid, 0.01, rel_tol.max(1e-3));

    ChamferCheck {
        mesh_volume,
        expected_volume,
        edge_length,
        wedge_ok,
        watertight,
        all_hold: wedge_ok && watertight,
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> Option<SolidId> {
    TopologyBuilder::new(model).create_box_3d(w, h, d).ok()?;
    model.solids.iter().last().map(|(id, _)| id)
}

fn failed() -> ChamferCheck {
    ChamferCheck {
        mesh_volume: None,
        expected_volume: 0.0,
        edge_length: 0.0,
        wedge_ok: false,
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
    fn chamfered_box_edge_removes_the_wedge_and_stays_watertight() {
        // 4×4×4 box, edge length 4, setback 1 → wedge ½·1·4 = 2; V = 64 − 2 = 62.
        let c = chamfer_box_edge_invariants(4.0, 4.0, 4.0, 1.0, 2e-2);
        assert!(c.wedge_ok, "{c:?}");
        assert!(c.watertight, "chamfered box not watertight: {c:?}");
        assert!((c.expected_volume - 62.0).abs() < 0.5, "{c:?}");
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

        /// V(chamfer) = V(box) − ½·d²·L, and the result is watertight, for a
        /// range of box sizes and setbacks (setback kept well below the box).
        #[test]
        fn pp_chamfer_wedge_volume(
            w in 3.0f64..10.0,
            h in 3.0f64..10.0,
            d in 3.0f64..10.0,
            dist in 0.3f64..1.5,
        ) {
            let c = chamfer_box_edge_invariants(w, h, d, dist, 3e-2);
            prop_assert!(c.wedge_ok, "w={w} h={h} d={d} dist={dist}: {c:?}");
            prop_assert!(c.watertight, "not watertight: {c:?}");
        }
    }
}
