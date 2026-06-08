//! Curved-boolean poke ENVELOPE gate (#85).
//!
//! A sphere whose centre sits ON a box face, poking half-in, is the canonical
//! curved face-pair ∩: the result is a hemisphere closed by a flat disk cap, and
//! it must be volume-exact ((2/3)π·r³) AND watertight. This gate pins the
//! *general* (non-degenerate) regime as correct across a sweep of radii and a
//! few face/axis placements — the regression lock for the curved classify +
//! side-wall stitch that the #82 diagnosis proved already works here.
//!
//! Coverage runs up to and INCLUDING the EXACT edge-tangency (r = box
//! half-width: the cut circle tangent to all four host edges) — fixed by the
//! densified interior-point containment in `compute_split_face_interior_points`
//! plus the tangential-contact rejection in `compute_edge_intersections` (a
//! tangency does not separate cells, so it must not split the arrangement). Only
//! the BEYOND-tangent regime (circle radius > box half-width, so it genuinely
//! crosses the host edges and the sphere is clipped by the adjacent box faces)
//! remains open — a multi-face transversal clip, not a tangency degeneracy,
//! tracked by the ignored `diag_sphere_poke_82` diagnostic.

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::Matrix4;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn the_box(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        o => panic!("box: {o:?}"),
    }
}

fn sphere(model: &mut BRepModel, c: [f64; 3], r: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(c[0], c[1], c[2]), r)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        o => panic!("sphere: {o:?}"),
    }
}

/// Half-sphere truth: the sphere centre is on the face plane, so exactly half is
/// inside the box (assuming r leaves the cut circle clear of the box edges).
fn half_sphere(r: f64) -> f64 {
    2.0 / 3.0 * std::f64::consts::PI * r * r * r
}

fn assert_poke_exact(center: [f64; 3], r: f64, label: &str) {
    let mut model = BRepModel::new();
    let bx = the_box(&mut model);
    let sp = sphere(&mut model, center, r);
    let result = boolean_operation(
        &mut model,
        bx,
        sp,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .unwrap_or_else(|e| panic!("{label}: boolean errored: {e:?}"));

    let vol = model
        .calculate_solid_volume(result)
        .unwrap_or_else(|| panic!("{label}: no volume"));
    let truth = half_sphere(r);
    let rel = (vol - truth).abs() / truth;
    assert!(
        rel <= 0.02,
        "{label}: vol {vol:.5} vs truth {truth:.5} ({:+.2}%)",
        100.0 * (vol - truth) / truth
    );

    let report = manifold_report(&model, result, 0.04, 1e-6)
        .unwrap_or_else(|| panic!("{label}: empty tessellation"));
    assert!(
        report.boundary_edges == 0 && report.closed,
        "{label}: not watertight — boundary_edges={} closed={}",
        report.boundary_edges,
        report.closed
    );
    assert!(
        report.nonmanifold_edges == 0,
        "{label}: non-manifold edges={}",
        report.nonmanifold_edges
    );
}

#[test]
fn sphere_poke_plus_x_face_radius_sweep_is_exact_and_watertight() {
    // Centre on the +x face (x=1); cut circle of radius r. The sweep runs from a
    // clear gap, through the near-tangent band, up to and INCLUDING the exact
    // edge-tangency r=1.0 (circle tangent to all four box edges) — every one must
    // be volume-exact and watertight.
    for &r in &[
        0.3_f64, 0.5, 0.6, 0.7, 0.8, 0.9, 0.95, 0.97, 0.99, 0.995, 1.0,
    ] {
        assert_poke_exact([1.0, 0.0, 0.0], r, &format!("+x poke r={r}"));
    }
}

#[test]
fn sphere_poke_is_exact_on_every_box_face() {
    let r = 0.8;
    let faces = [
        ([1.0, 0.0, 0.0], "+x"),
        ([-1.0, 0.0, 0.0], "-x"),
        ([0.0, 1.0, 0.0], "+y"),
        ([0.0, -1.0, 0.0], "-y"),
        ([0.0, 0.0, 1.0], "+z"),
        ([0.0, 0.0, -1.0], "-z"),
    ];
    for (c, name) in faces {
        assert_poke_exact(c, r, &format!("{name} face poke"));
    }
}

#[test]
fn sphere_poke_off_centre_on_face_is_exact() {
    // Sphere centre on the +x face but shifted in y,z — still half in, cut circle
    // (r=0.6) clear of the edges. Guards against an axis-aligned-only fix.
    for &c in &[[1.0, 0.3, 0.0], [1.0, 0.0, 0.3], [1.0, -0.25, 0.25]] {
        assert_poke_exact(c, 0.6, &format!("off-centre poke {c:?}"));
    }
}

#[test]
fn sphere_poke_survives_box_rotation() {
    // Rotate the box 30° about z, sphere poking the (rotated) +y face along the
    // rotated normal. Exercises the curved-classify on a non-axis-aligned plane.
    let mut model = BRepModel::new();
    let bx = the_box(&mut model);
    let ang = 30.0_f64.to_radians();
    transform_solid(
        &mut model,
        bx,
        Matrix4::rotation_z(ang),
        TransformOptions::default(),
    )
    .expect("rotate box");
    // Rotated +y face centre is at R·(0,1,0) = (-sin, cos, 0).
    let n = Vector3::new(-ang.sin(), ang.cos(), 0.0);
    let c = [n.x, n.y, n.z];
    let r = 0.7;
    let sp = sphere(&mut model, c, r);
    let result = boolean_operation(
        &mut model,
        bx,
        sp,
        BooleanOp::Intersection,
        BooleanOptions::default(),
    )
    .expect("rotated poke boolean");
    let vol = model.calculate_solid_volume(result).expect("vol");
    let truth = half_sphere(r);
    assert!(
        (vol - truth).abs() / truth <= 0.02,
        "rotated poke: vol {vol:.5} vs truth {truth:.5}"
    );
    let rep = manifold_report(&model, result, 0.04, 1e-6).expect("mesh");
    assert!(
        rep.boundary_edges == 0 && rep.closed,
        "rotated poke not watertight: {rep:?}"
    );
}
