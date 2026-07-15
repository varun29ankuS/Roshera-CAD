// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Determinism net for the tessellation pipeline (P0 #84).
//!
//! The mesh is what ships to export and the viewport, so it must be byte-stable
//! run-to-run — a per-process `HashMap` iteration order must never change which
//! triangles come out or in what order (the #69 normal-weld class of bug).
//!
//! `tessellate_solid` is read-only and builds its own internal maps each call,
//! which `std::HashMap` reseeds per call, so tessellating the SAME solid N times
//! in one process exercises N different internal iteration orders. Every call
//! must produce an identical mesh — same vertex/triangle counts and the same
//! vertices and indices in the same order.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

/// Order-sensitive fingerprint of the tessellated mesh: (#vertices, #triangles,
/// FNV-1a hash over every vertex position bit-pattern and every triangle index,
/// in emission order). Two tessellations are byte-identical iff their signatures
/// match.
fn tess_signature(model: &BRepModel, solid_id: SolidId) -> (usize, usize, u64) {
    let solid = model.solids.get(solid_id).expect("solid stored");
    let mesh = tessellate_solid(solid, model, &TessellationParams::default());

    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    let mut mix = |x: u64| {
        h ^= x;
        h = h.wrapping_mul(0x100000001b3);
    };
    for v in &mesh.vertices {
        mix(v.position.x.to_bits());
        mix(v.position.y.to_bits());
        mix(v.position.z.to_bits());
    }
    for t in &mesh.triangles {
        mix(t[0] as u64);
        mix(t[1] as u64);
        mix(t[2] as u64);
    }
    (mesh.vertices.len(), mesh.triangles.len(), h)
}

fn assert_tessellation_deterministic(label: &str, model: &BRepModel, solid_id: SolidId) {
    let sigs: Vec<(usize, usize, u64)> = (0..8).map(|_| tess_signature(model, solid_id)).collect();
    let first = sigs[0];
    for (i, s) in sigs.iter().enumerate() {
        assert_eq!(
            *s, first,
            "{label}: tessellation is non-deterministic — run 0 = {first:?}, run {i} = {s:?}"
        );
    }
}

fn box_solid(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn sphere_solid(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 1.0)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn cylinder_solid(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, -1.0),
            Vector3::new(0.0, 0.0, 1.0),
            1.0,
            2.0,
        )
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

#[test]
fn box_tessellation_is_deterministic() {
    let mut model = BRepModel::new();
    let s = box_solid(&mut model);
    assert_tessellation_deterministic("box", &model, s);
}

#[test]
fn sphere_tessellation_is_deterministic() {
    let mut model = BRepModel::new();
    let s = sphere_solid(&mut model);
    assert_tessellation_deterministic("sphere", &model, s);
}

#[test]
fn cylinder_tessellation_is_deterministic() {
    let mut model = BRepModel::new();
    let s = cylinder_solid(&mut model);
    assert_tessellation_deterministic("cylinder", &model, s);
}

// --- Fillet / chamfer determinism --------------------------------------------
//
// These ops mutate the model, so each run rebuilds a fresh cube, picks the same
// (deterministically chosen) edge, blends it, and measures the result volume.
// 8 runs in one process exercise 8 internal hash seeds; the volume must be
// stable to 1e-6 relative (gross/topological non-determinism — a different blend
// result — moves it far more; sub-1e-6 FP-summation noise is tolerated).

#[path = "blend_fixtures/mod.rs"]
mod blend_fixtures;
use blend_fixtures::{make_cube, vertex_at};
use geometry_engine::operations::chamfer::{
    chamfer_edges, ChamferOptions, ChamferType, PropagationMode as ChamferProp,
};
use geometry_engine::operations::fillet::{FilletType, PropagationMode as FilletProp};
use geometry_engine::operations::{fillet_edges, CommonOptions, FilletOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::vertex::VertexId;

const BOX_SIZE: f64 = 10.0;
const HALF_BOX: f64 = BOX_SIZE / 2.0;
const RADIUS: f64 = 0.5;
const OFFSET: f64 = 0.5;

/// Min-id edge incident to `vertex` — deterministic by construction so the same
/// edge is blended on every run; any run-to-run drift is then the blend's fault.
fn pick_one_edge_at_vertex(model: &BRepModel, vertex: VertexId) -> EdgeId {
    model
        .edges
        .iter()
        .filter_map(|(id, edge)| {
            if edge.start_vertex == vertex || edge.end_vertex == vertex {
                Some(id)
            } else {
                None
            }
        })
        .min()
        .expect("at least one edge incident to vertex")
}

fn fillet_opts() -> FilletOptions {
    FilletOptions {
        fillet_type: FilletType::Constant(RADIUS),
        radius: RADIUS,
        propagation: FilletProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn chamfer_opts() -> ChamferOptions {
    ChamferOptions {
        chamfer_type: ChamferType::EqualDistance(OFFSET),
        distance1: OFFSET,
        distance2: OFFSET,
        symmetric: true,
        propagation: ChamferProp::None,
        common: CommonOptions {
            validate_result: true,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn blended_volume(blend: impl Fn(&mut BRepModel, SolidId, EdgeId)) -> f64 {
    let mut model = BRepModel::new();
    let solid = make_cube(&mut model, BOX_SIZE);
    let corner = vertex_at(&model, HALF_BOX, HALF_BOX, HALF_BOX);
    let edge = pick_one_edge_at_vertex(&model, corner);
    blend(&mut model, solid, edge);
    model.calculate_solid_volume(solid).expect("volume")
}

fn assert_blend_deterministic(label: &str, blend: impl Fn(&mut BRepModel, SolidId, EdgeId)) {
    let runs: Vec<f64> = (0..8).map(|_| blended_volume(&blend)).collect();
    let first = runs[0];
    for (i, &v) in runs.iter().enumerate() {
        assert!(
            (v - first).abs() / first.abs().max(1.0) < 1e-6,
            "{label} non-deterministic: run 0 = {first}, run {i} = {v} (all = {runs:?})"
        );
    }
}

#[test]
fn fillet_is_deterministic() {
    assert_blend_deterministic("fillet", |m, s, e| {
        fillet_edges(m, s, vec![e], fillet_opts()).expect("fillet");
    });
}

#[test]
fn chamfer_is_deterministic() {
    assert_blend_deterministic("chamfer", |m, s, e| {
        chamfer_edges(m, s, vec![e], chamfer_opts()).expect("chamfer");
    });
}

// --- Extrude determinism -----------------------------------------------------

use geometry_engine::operations::extrude::{extrude_profile, ExtrudeOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};

/// Extrude a unit square profile into a box, return its volume. Fresh model per
/// call (extrude mutates).
fn extrude_box_volume() -> f64 {
    let mut model = BRepModel::new();
    let verts = [
        Point3::new(-1.0, -1.0, 0.0),
        Point3::new(1.0, -1.0, 0.0),
        Point3::new(1.0, 1.0, 0.0),
        Point3::new(-1.0, 1.0, 0.0),
    ];
    let v_ids: Vec<VertexId> = verts
        .iter()
        .map(|p| model.vertices.add(p.x, p.y, p.z))
        .collect();
    let mut edges = Vec::with_capacity(4);
    for i in 0..4 {
        let line = Line::new(verts[i], verts[(i + 1) % 4]);
        let cid = model.curves.add(Box::new(line));
        let e = Edge::new_auto_range(
            0,
            v_ids[i],
            v_ids[(i + 1) % 4],
            cid,
            EdgeOrientation::Forward,
        );
        edges.push(model.edges.add(e));
    }
    let opts = ExtrudeOptions {
        direction: Vector3::Z,
        distance: 3.0,
        cap_ends: true,
        ..ExtrudeOptions::default()
    };
    let solid = extrude_profile(&mut model, edges, opts).expect("extrude");
    model.calculate_solid_volume(solid).expect("volume")
}

#[test]
fn extrude_is_deterministic() {
    let runs: Vec<f64> = (0..8).map(|_| extrude_box_volume()).collect();
    let first = runs[0];
    for (i, &v) in runs.iter().enumerate() {
        assert!(
            (v - first).abs() / first.abs().max(1.0) < 1e-6,
            "extrude non-deterministic: run 0 = {first}, run {i} = {v} (all = {runs:?})"
        );
    }
}

// --- Revolve / Sweep / Loft determinism --------------------------------------
//
// These ops mutate the model and build SHARED vertex/edge grids (revolve rings,
// sweep stations, loft correspondence) whose assembly walks internal maps. Each
// run rebuilds a fresh model, runs the op, and measures the result volume; 8
// in-process runs (8 internal hash seeds) must agree to 1e-6 relative. A
// different grid assembly or a dropped face moves the volume far more than that;
// sub-1e-6 FP-summation noise is tolerated.

use geometry_engine::operations::{
    loft_profiles, revolve_profile, sweep_profile, LoftOptions, RevolveOptions, SweepOptions,
};

fn line_edge_between(model: &mut BRepModel, a: VertexId, b: VertexId) -> EdgeId {
    let pa = model.vertices.get(a).expect("a").position;
    let pb = model.vertices.get(b).expect("b").position;
    let cid = model
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    model
        .edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

fn assert_volume_run_deterministic(label: &str, run: impl Fn() -> f64) {
    let runs: Vec<f64> = (0..8).map(|_| run()).collect();
    let first = runs[0];
    for (i, &v) in runs.iter().enumerate() {
        assert!(
            (v - first).abs() / first.abs().max(1.0) < 1e-6,
            "{label} non-deterministic: run 0 = {first}, run {i} = {v} (all = {runs:?})"
        );
    }
}

/// Revolve a rectangle (x ∈ [1,2], z ∈ [0,3]) a half-turn about the Z axis.
/// The half revolution exercises both the swept walls and the two end caps.
fn revolve_volume_run() -> f64 {
    let mut model = BRepModel::new();
    let v0 = model.vertices.add(1.0, 0.0, 0.0);
    let v1 = model.vertices.add(2.0, 0.0, 0.0);
    let v2 = model.vertices.add(2.0, 0.0, 3.0);
    let v3 = model.vertices.add(1.0, 0.0, 3.0);
    let edges = vec![
        line_edge_between(&mut model, v0, v1),
        line_edge_between(&mut model, v1, v2),
        line_edge_between(&mut model, v2, v3),
        line_edge_between(&mut model, v3, v0),
    ];
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: std::f64::consts::PI,
        ..RevolveOptions::default()
    };
    let solid = revolve_profile(&mut model, edges, opts).expect("revolve");
    model.calculate_solid_volume(solid).expect("volume")
}

/// Sweep a 2×3 rectangle along +Z by 4.
fn sweep_volume_run() -> f64 {
    let mut model = BRepModel::new();
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(2.0, 0.0, 0.0);
    let v2 = model.vertices.add(2.0, 3.0, 0.0);
    let v3 = model.vertices.add(0.0, 3.0, 0.0);
    let profile = vec![
        line_edge_between(&mut model, v0, v1),
        line_edge_between(&mut model, v1, v2),
        line_edge_between(&mut model, v2, v3),
        line_edge_between(&mut model, v3, v0),
    ];
    let pa = model.vertices.add(0.0, 0.0, 0.0);
    let pb = model.vertices.add(0.0, 0.0, 4.0);
    let path = line_edge_between(&mut model, pa, pb);
    let solid = sweep_profile(&mut model, profile, path, SweepOptions::default()).expect("sweep");
    model.calculate_solid_volume(solid).expect("volume")
}

/// Loft between two axis-aligned squares of different size at z = 0 and z = 20.
fn loft_volume_run() -> f64 {
    let mut model = BRepModel::new();
    let square = |model: &mut BRepModel, z: f64, side: f64| -> Vec<EdgeId> {
        let h = side / 2.0;
        let v0 = model.vertices.add(-h, -h, z);
        let v1 = model.vertices.add(h, -h, z);
        let v2 = model.vertices.add(h, h, z);
        let v3 = model.vertices.add(-h, h, z);
        vec![
            line_edge_between(model, v0, v1),
            line_edge_between(model, v1, v2),
            line_edge_between(model, v2, v3),
            line_edge_between(model, v3, v0),
        ]
    };
    let p0 = square(&mut model, 0.0, 10.0);
    let p1 = square(&mut model, 20.0, 6.0);
    let opts = LoftOptions {
        create_solid: true,
        ..Default::default()
    };
    let solid = loft_profiles(&mut model, vec![p0, p1], opts).expect("loft");
    model.calculate_solid_volume(solid).expect("volume")
}

#[test]
fn revolve_is_deterministic() {
    assert_volume_run_deterministic("revolve", revolve_volume_run);
}

#[test]
fn sweep_is_deterministic() {
    assert_volume_run_deterministic("sweep", sweep_volume_run);
}

#[test]
fn loft_is_deterministic() {
    assert_volume_run_deterministic("loft", loft_volume_run);
}
