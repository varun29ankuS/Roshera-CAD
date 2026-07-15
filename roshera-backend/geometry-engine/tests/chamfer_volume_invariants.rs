// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Oracle-free volume + watertightness invariants for the chamfer operation.
//!
//! An equal-distance (45°) chamfer of setback `d` on a single straight convex
//! edge of length `L` removes a right-triangular prism: cross-section area
//! `d²/2`, length `L`, so the removed volume is `(d²/2)·L`. Chamfering one edge
//! of a `W×H×D` box therefore leaves volume `W·H·D − (d²/2)·L`. (The prism's
//! end caps lie in the box's end faces; with only one edge chamfered there is
//! no corner interaction, so this is exact.)
//!
//! Existing chamfer tests (blend_topology_regression.rs) check topology only
//! (no boundary edges, Euler V−E+F=2, validator passes) — never the enclosed
//! volume. These assert the geometry directly: the result is validated at
//! construction, and its tessellated divergence volume must equal both the
//! analytic post-chamfer volume AND the reported mass-properties volume (the
//! watertightness witness). The edge length `L` is measured from the chamfered
//! edge's own vertices, so the oracle does not assume box-edge ordering.
//!
//! A chamfer that returns a typed `Err` on this trivial known-good input is a
//! regression, not a skip — the helper `.expect()`s success.

use geometry_engine::operations::chamfer::{ChamferType, PropagationMode};
use geometry_engine::operations::{chamfer_edges, ChamferOptions, CommonOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b.abs() < 1e-9 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize].position;
        let b = mesh.vertices[t[1] as usize].position;
        let c = mesh.vertices[t[2] as usize].position;
        v += (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x))
            / 6.0;
    }
    v.abs()
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(w, h, d)
        .expect("create_box_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

/// Chamfer the first edge of a `w×h×d` box by equal setback `dist`. Returns
/// (chamfered-edge length L, tessellated divergence volume, reported mass-props
/// volume).
fn chamfer_first_edge(w: f64, h: f64, d: f64, dist: f64) -> (f64, f64, f64) {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model, w, h, d);

    let edge_id = model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box must have edges");
    // Measure the chamfered edge's length from its own endpoints.
    let edge = model.edges.get(edge_id).expect("edge").clone();
    let p0 = model.vertices.get(edge.start_vertex).expect("v0").position;
    let p1 = model.vertices.get(edge.end_vertex).expect("v1").position;
    let length =
        ((p0[0] - p1[0]).powi(2) + (p0[1] - p1[1]).powi(2) + (p0[2] - p1[2]).powi(2)).sqrt();

    let opts = ChamferOptions {
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
    chamfer_edges(&mut model, solid_id, vec![edge_id], opts)
        .expect("equal-distance chamfer of a single box edge must succeed");

    let mp = model
        .mass_properties_for(solid_id)
        .expect("chamfered solid mass properties");
    let solid = model.solids.get(solid_id).expect("chamfered solid");
    let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
    (length, mesh_volume(&mesh), mp.volume)
}

macro_rules! chamfer_volume_test {
    ($name:ident, $w:expr, $h:expr, $d:expr, $dist:expr) => {
        #[test]
        fn $name() {
            let (length, tess_vol, mass_vol) = chamfer_first_edge($w, $h, $d, $dist);
            let box_vol = ($w as f64) * ($h as f64) * ($d as f64);
            let removed = 0.5 * ($dist as f64) * ($dist as f64) * length;
            let expected = box_vol - removed;
            // Analytic wedge oracle.
            assert!(
                rel_close(tess_vol, expected, 0.02),
                "chamfer {}x{}x{} d={} on edge len {}: tess volume {} vs analytic {} (removed {})",
                $w,
                $h,
                $d,
                $dist,
                length,
                tess_vol,
                expected,
                removed
            );
            // Watertightness witness.
            assert!(
                rel_close(tess_vol, mass_vol, 0.02),
                "chamfer {}x{}x{} d={}: tess {} vs mass-props {} (non-watertight?)",
                $w,
                $h,
                $d,
                $dist,
                tess_vol,
                mass_vol
            );
        }
    };
}

chamfer_volume_test!(chamfer_cube10_d1, 10.0, 10.0, 10.0, 1.0);
chamfer_volume_test!(chamfer_cube10_d2, 10.0, 10.0, 10.0, 2.0);
chamfer_volume_test!(chamfer_8_6_10_d1, 8.0, 6.0, 10.0, 1.0);
chamfer_volume_test!(chamfer_cube6_d05, 6.0, 6.0, 6.0, 0.5);

#[test]
fn chamfer_removes_material_monotonically_in_setback() {
    let (_, small, _) = chamfer_first_edge(10.0, 10.0, 10.0, 1.0);
    let (_, large, _) = chamfer_first_edge(10.0, 10.0, 10.0, 2.0);
    // A larger setback removes more material ⇒ smaller remaining volume.
    assert!(
        large < small,
        "larger chamfer setback must remove more material: d=2 gave {large} !< d=1 {small}"
    );
}

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Wedge oracle over randomized box sizes and setbacks. Box dims are kept
    /// ≥ 4 and the setback ≤ 1.2, so the chamfer of the first edge stays a
    /// clean triangular prism (setback ≪ edge/2 ⇒ no corner interaction).
    /// Every case is validated at construction (via `chamfer_first_edge`,
    /// `validate_result:true`), checked against the wedge volume, and checked
    /// for watertightness.
    #[test]
    fn prop_chamfer_wedge_volume(
        w in 4.0f64..10.0,
        h in 4.0f64..10.0,
        d in 4.0f64..10.0,
        dist in 0.2f64..1.2,
    ) {
        let (length, tess, mass) = chamfer_first_edge(w, h, d, dist);
        let expected = w * h * d - 0.5 * dist * dist * length;
        prop_assert!(rel_close(tess, expected, 0.03), "tess {tess} vs wedge {expected}");
        prop_assert!(rel_close(tess, mass, 0.03), "tess {tess} vs mass-props {mass}");
    }
}
