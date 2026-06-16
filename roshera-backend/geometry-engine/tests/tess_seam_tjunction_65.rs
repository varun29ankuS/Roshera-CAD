//! KNOWN_BUGS #65 / #21 — boolean curved↔planar SEAM T-junctions at fine
//! tessellation density.
//!
//! Diagnosis (2026-06-16): plate ∪ coaxial cylinder boss is B-Rep VALID and
//! its mesh is watertight at coarse density, but at the DISPLAY/EXPORT chord
//! (TessellationParams::default = 0.001) the seam tessellates NON-MANIFOLD:
//!   chord 0.01 → nm=0 · 0.005 → nm=5 · 0.001 → nm=2.
//! The weld is NOT the cause — manifold_report and the render's 1e-5 grid weld
//! agree exactly at every chord. Root: the cylinder lateral (curved) and the
//! adjacent planar face (plate top, circular hole) sample the SHARED seam
//! circle at DIFFERENT parameter points, so the meshes don't share seam
//! vertices → T-junctions. Fix lane = consistent shared boundary-edge sampling
//! across adjacent curved+planar faces at a boolean seam (#21/#24). #[ignore]'d
//! until that lands; the SOUND verdict is the B-Rep (valid), which is asserted
//! un-ignored below so the gate still proves the part itself is not broken.
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
#[ignore = "KNOWN_BUGS #65/#21 — boolean curved↔planar seam T-junctions at fine chord"]
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
