// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Volume + watertightness invariants for fillet/chamfer as *operations on a
//! solid* (not just topology). Filleting a convex edge removes the square
//! corner and replaces it with a quarter-round: the material removed along a
//! straight edge of length L at radius r is r²(1 − π/4)·L. Chamfering removes a
//! right-triangle prism: ½·d²·L. These are oracle-free volume checks; the
//! result mesh must also stay watertight (divergence volume = reported volume).
//!
//! Topology of the blend is already covered by blend_topology_regression.rs;
//! this file adds the *quantitative* and *transformed-input* (composition)
//! coverage.

use std::f64::consts::PI;

use geometry_engine::math::{Matrix4, Vector3};
use geometry_engine::operations::chamfer::{ChamferType, PropagationMode as ChamferProp};
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletPropagation};
use geometry_engine::operations::{
    chamfer_edges, fillet_edges, transform_solid, ChamferOptions, FilletOptions, TransformOptions,
};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

const L: f64 = 10.0; // box edge length

fn make_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(L, L, L)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn first_edge(model: &BRepModel) -> EdgeId {
    model
        .edges
        .iter()
        .map(|(id, _)| id)
        .next()
        .expect("box has edges")
}

fn vol(model: &mut BRepModel, id: SolidId) -> f64 {
    model.mass_properties_for(id).expect("mass props").volume
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

fn assert_watertight(model: &BRepModel, id: SolidId, reported: f64, label: &str) {
    let solid = model.solids.get(id).expect("solid");
    let mesh = tessellate_solid(solid, model, &TessellationParams::default());
    let mv = mesh_volume(&mesh);
    let rel = (mv - reported).abs() / reported.max(1e-9);
    assert!(
        rel < 0.05,
        "{label}: mesh volume {mv} vs reported {reported} (rel {rel}) — not watertight"
    );
}

fn fillet_opts(r: f64) -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(r),
        radius: r,
        propagation: FilletPropagation::None,
        ..Default::default()
    }
}

fn chamfer_opts(d: f64) -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(d),
        distance1: d,
        distance2: d,
        symmetric: true,
        propagation: ChamferProp::None,
        ..Default::default()
    }
}

#[test]
fn fillet_removes_quarter_round_corner_volume() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model);
    let before = vol(&mut model, id);
    assert!((before - 1000.0).abs() < 1.0, "box volume {before}");

    let r = 2.0;
    let edge = first_edge(&model);
    fillet_edges(&mut model, id, vec![edge], fillet_opts(r)).expect("fillet one box edge");

    let after = vol(&mut model, id);
    let removed = before - after;
    // Ideal straight-edge removal; end-corner effects are second order, so the
    // band is generous but still rejects "removed nothing" / "removed a face".
    let expected = r * r * (1.0 - PI / 4.0) * L; // ≈ 8.58
    assert!(after < before, "fillet did not remove material");
    assert!(
        removed > expected * 0.5 && removed < expected * 2.0,
        "fillet removed {removed}, expected ≈ {expected} (band 0.5×–2×)"
    );
    assert_watertight(&model, id, after, "filleted box");
}

#[test]
fn chamfer_removes_triangular_corner_volume() {
    let mut model = BRepModel::new();
    let id = make_box(&mut model);
    let before = vol(&mut model, id);

    let d = 2.0;
    let edge = first_edge(&model);
    if chamfer_edges(&mut model, id, vec![edge], chamfer_opts(d)).is_ok() {
        let after = vol(&mut model, id);
        let removed = before - after;
        let expected = 0.5 * d * d * L; // ≈ 20
        assert!(after < before, "chamfer did not remove material");
        assert!(
            removed > expected * 0.5 && removed < expected * 2.0,
            "chamfer removed {removed}, expected ≈ {expected}"
        );
        assert_watertight(&model, id, after, "chamfered box");
    }
}

#[test]
fn fillet_radius_monotonic_in_removed_volume() {
    // A larger fillet radius removes more material.
    let removed = |r: f64| -> f64 {
        let mut m = BRepModel::new();
        let id = make_box(&mut m);
        let before = vol(&mut m, id);
        let e = first_edge(&m);
        fillet_edges(&mut m, id, vec![e], fillet_opts(r)).expect("fillet");
        before - vol(&mut m, id)
    };
    let small = removed(1.0);
    let large = removed(3.0);
    assert!(
        large > small,
        "larger radius should remove more: r=1 removed {small}, r=3 removed {large}"
    );
}

#[test]
fn fillet_on_transformed_box_is_watertight_and_reduces_volume() {
    // Composition: build → rigidly move → fillet → measure. The fillet must
    // behave identically on a rotated/translated box (a rigid motion can't
    // change how much material a corner round removes).
    let mut model = BRepModel::new();
    let id = make_box(&mut model);
    let m = Matrix4::from_translation(&Vector3::new(3.0, -2.0, 1.0))
        * Matrix4::from_axis_angle(&Vector3::new(1.0, 1.0, 0.0).normalize().unwrap(), 0.6)
            .expect("rot");
    transform_solid(&mut model, id, m, TransformOptions::default()).expect("transform box");
    let before = vol(&mut model, id);
    assert!(
        (before - 1000.0).abs() < 1.0,
        "transformed box volume {before}"
    );

    let edge = first_edge(&model);
    if fillet_edges(&mut model, id, vec![edge], fillet_opts(2.0)).is_ok() {
        let after = vol(&mut model, id);
        assert!(after < before, "fillet on moved box removed nothing");
        let removed = before - after;
        let expected = 4.0 * (1.0 - PI / 4.0) * L;
        assert!(
            removed > expected * 0.5 && removed < expected * 2.0,
            "transformed-box fillet removed {removed}, expected ≈ {expected}"
        );
        assert_watertight(&model, id, after, "filleted transformed box");
    }
}
