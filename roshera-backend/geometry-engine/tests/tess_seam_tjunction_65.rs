// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! KNOWN_BUGS #65 — boolean-result curved face doubled-facet → non-manifold
//! mesh at fine density. FIXED 2026-06-16 (doubled-facet removal in
//! `weld_mesh_watertight_range`).
//!
//! plate ∪ coaxial cylinder boss is B-Rep VALID, but at the DISPLAY/EXPORT
//! chord (TessellationParams::default = 0.001) the mesh was NON-MANIFOLD
//! (chord 0.01→nm=0 · 0.005→nm=5 · 0.001→nm=2). Localized: NOT the boolean
//! seam and NOT the bare curved-CDT (a plain cylinder is manifold at every
//! chord) — the curved-CDT of the BOOLEAN-RESULT lateral emitted a degenerate
//! sliver TWICE with opposite winding (a "doubled facet"/fin: same 3 welded
//! vertices, area ~0.002), so every fin edge bordered 4 triangles. Fix: the
//! weld pass now cancels opposite-winding facet pairs + dedups same-winding
//! duplicates (no-op on clean meshes). Both gates below are live.
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn build(m: &mut BRepModel) -> u32 {
    let plate = match TopologyBuilder::new(m)
        .create_box_3d(120.0, 80.0, 16.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    let boss = match TopologyBuilder::new(m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, -4.0), Vector3::Z, 26.0, 45.0)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    };
    boolean_operation(m, plate, boss, BooleanOp::Union, BooleanOptions::default()).unwrap()
}

#[test]
fn box_union_cylinder_brep_is_sound_65() {
    // The SOUND fact: the union is a valid closed B-Rep (mesh-independent).
    let mut m = BRepModel::new();
    let sid = build(&mut m);
    assert!(
        validate_solid_scoped(&m, sid, Tolerance::default(), ValidationLevel::Standard).is_valid,
        "box ∪ coaxial cylinder must be a valid B-Rep"
    );
}

#[test]
fn box_union_cylinder_fine_mesh_watertight_65() {
    let mut m = BRepModel::new();
    let sid = build(&mut m);
    let r = manifold_report(&m, sid, 0.001, 1e-6).expect("report");
    assert_eq!(
        (r.boundary_edges, r.nonmanifold_edges),
        (0, 0),
        "fine display/export mesh must be watertight (seam shared-edge sampling)"
    );
}
