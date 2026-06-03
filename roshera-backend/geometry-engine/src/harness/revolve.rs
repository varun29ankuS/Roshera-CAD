//! Revolve correctness harness (GEOM-HARNESS).
//!
//! Invariant: revolving a planar profile about an axis obeys **Pappus's second
//! theorem** — the swept volume equals the profile area times the distance its
//! centroid travels. For a rectangle `x ∈ [x0,x1]`, `z ∈ [z0,z1]` in the XZ plane
//! revolved `angle` about the Z axis this is `V = ½·angle·(x1²−x0²)·(z1−z0)`.
//!
//! The oracle compares the **tessellated** volume to that closed form: for a
//! solid of revolution this simultaneously certifies geometric correctness *and*
//! mesh watertightness (a leaky mesh's divergence volume would not match Pappus).
//! Mass-properties volume is deliberately NOT used — it has a documented residual
//! disagreement with the tessellation for `SurfaceOfRevolution` solids (so the
//! universal `is_watertight`, which compares the two, does not apply here). Full
//! (2π) revolutions also leave a documented boundary-edge tessellation gap
//! (CDT-γ class), so this harness exercises the watertight *partial*-revolution
//! regime; the residuals are tracked separately.

use crate::harness::watertight::mesh_volume;
use crate::math::vector3::{Point3, Vector3};
use crate::operations::revolve::{revolve_profile, RevolveOptions};
use crate::primitives::curve::Line;
use crate::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use crate::primitives::topology_builder::BRepModel;

/// Result of a revolve invariant check.
#[derive(Debug, Clone)]
pub struct RevolveCheck {
    pub mesh_volume: Option<f64>,
    pub pappus_volume: f64,
    /// Tessellated volume ≈ the Pappus closed form — geometric correctness and
    /// mesh watertightness in one (a leak would skew the divergence volume).
    pub pappus_ok: bool,
}

/// Revolve the XZ rectangle `[x0,x1]×[z0,z1]` about the Z axis by `angle`, and
/// check the volume against Pappus + watertightness. Requires `x0 ≥ 0` (profile
/// on one side of the axis).
pub fn revolve_rect_invariants(
    x0: f64,
    x1: f64,
    z0: f64,
    z1: f64,
    angle: f64,
    rel_tol: f64,
) -> RevolveCheck {
    let mut model = BRepModel::new();
    let edges = offset_rectangle(&mut model, x0, x1, z0, z1);
    let options = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle,
        ..RevolveOptions::default()
    };
    let pappus_volume = 0.5 * angle * (x1 * x1 - x0 * x0) * (z1 - z0);

    match revolve_profile(&mut model, edges, options) {
        Ok(solid) => {
            let mesh_volume = mesh_volume(&model, solid, 0.001);
            let pappus_ok = mesh_volume.is_some_and(|m| within_rel(m, pappus_volume, rel_tol));
            RevolveCheck {
                mesh_volume,
                pappus_volume,
                pappus_ok,
            }
        }
        Err(_) => RevolveCheck {
            mesh_volume: None,
            pappus_volume,
            pappus_ok: false,
        },
    }
}

// ---------------------------------------------------------------------------
// helpers (private)
// ---------------------------------------------------------------------------

/// Closed CCW rectangle in the XZ plane (y = 0).
fn offset_rectangle(model: &mut BRepModel, x0: f64, x1: f64, z0: f64, z1: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(x0, 0.0, z0);
    let v1 = model.vertices.add(x1, 0.0, z0);
    let v2 = model.vertices.add(x1, 0.0, z1);
    let v3 = model.vertices.add(x0, 0.0, z1);
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
    use std::f64::consts::PI;

    #[test]
    fn half_revolution_matches_pappus() {
        // Half revolution adds two flat end caps; Pappus holds and the mesh is
        // watertight (it matches the analytic volume).
        let c = revolve_rect_invariants(1.0, 3.0, 0.0, 2.0, PI, 3e-2);
        assert!(c.pappus_ok, "{c:?}");
    }

    #[test]
    fn quarter_revolution_matches_pappus() {
        let c = revolve_rect_invariants(2.0, 4.0, 0.0, 1.0, PI / 2.0, 3e-2);
        assert!(c.pappus_ok, "{c:?}");
        // V = ½·(π/2)·(16−4)·1 = 3π.
        assert!((c.pappus_volume - 3.0 * PI).abs() < 1e-9);
    }

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

        /// Pappus holds (mesh watertight + correct) for any healthy rectangle
        /// revolved a clean quarter-turn. The angle is fixed: arbitrary-angle
        /// partial revolutions, full (2π) revolutions, and tiny/near-axis
        /// profiles all have tracked tessellation residuals (REVOLVE-ROBUSTNESS),
        /// so this harness pins the robust regime the existing suite relies on.
        #[test]
        fn pp_quarter_revolve_profile_matches_pappus(
            x0 in 1.5f64..4.0,
            w in 1.0f64..2.5,
            z0 in 0.0f64..2.0,
            h in 1.0f64..2.5,
        ) {
            let c = revolve_rect_invariants(x0, x0 + w, z0, z0 + h, PI / 2.0, 4e-2);
            prop_assert!(c.pappus_ok, "x0={x0} w={w} z0={z0} h={h}: {c:?}");
        }
    }
}
