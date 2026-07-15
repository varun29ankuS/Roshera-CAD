// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Blind-spot audit (#11) — hunts perception lies.
//!
//! The dangerous failure class is a solid that one validator calls GOOD and
//! another calls BROKEN: the v1 flanged housing tessellated to a plausible mesh
//! while its B-Rep was non-watertight ("looks solid, is broken"). This gate
//! asserts the two independent soundness oracles AGREE across a multi-surface-
//! type battery, and that a real defect is caught by BOTH (neither is blind):
//!   * `validate_solid_scoped` — B-Rep structural validity (loops/edges/faces).
//!   * `manifold_report`       — MESH watertightness (boundary/non-manifold edges).
//! Plus tessellation determinism (same solid → byte-identical mesh stats).
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

/// (brep_valid, mesh_watertight) for a solid at a sane deflection.
fn oracles(m: &BRepModel, s: SolidId, size: f64) -> (bool, bool) {
    let brep =
        validate_solid_scoped(m, s, Tolerance::default(), ValidationLevel::Standard).is_valid;
    let defl = (size * 0.02).max(1e-4);
    let mesh = manifold_report(m, s, defl, 1e-6)
        .map(|r| r.boundary_edges == 0 && r.nonmanifold_edges == 0)
        .unwrap_or(false);
    (brep, mesh)
}

fn revolve_tube(m: &mut BRepModel) -> SolidId {
    let pts = [(5.0, 0.0), (10.0, 0.0), (10.0, 20.0), (5.0, 20.0)];
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    revolve_profile(
        m,
        edges,
        RevolveOptions {
            axis_origin: Point3::ZERO,
            axis_direction: Vector3::Z,
            angle: std::f64::consts::TAU,
            segments: 48,
            ..Default::default()
        },
    )
    .expect("tube")
}

#[test]
fn brep_and_mesh_oracles_agree_across_surface_types() {
    // Each entry builds one part; both oracles must call it GOOD. A divergence
    // (one true, one false) is a blind spot — exactly the "looks solid but
    // broken" class. size = characteristic dimension for the mesh deflection.
    let cases: Vec<(&str, Box<dyn Fn(&mut BRepModel) -> SolidId>, f64)> = vec![
        (
            "box",
            Box::new(|m: &mut BRepModel| {
                sid(TopologyBuilder::new(m)
                    .create_box_3d(40.0, 30.0, 20.0)
                    .expect("box"))
            }),
            40.0,
        ),
        (
            "cylinder",
            Box::new(|m: &mut BRepModel| {
                sid(TopologyBuilder::new(m)
                    .create_cylinder_3d(Point3::ZERO, Vector3::Z, 10.0, 30.0)
                    .expect("cyl"))
            }),
            30.0,
        ),
        (
            "sphere",
            Box::new(|m: &mut BRepModel| {
                sid(TopologyBuilder::new(m)
                    .create_sphere_3d(Point3::ZERO, 15.0)
                    .expect("sph"))
            }),
            30.0,
        ),
        (
            "frustum",
            Box::new(|m: &mut BRepModel| {
                sid(TopologyBuilder::new(m)
                    .create_cone_3d(Point3::ZERO, Vector3::Z, 12.0, 6.0, 20.0)
                    .expect("cone"))
            }),
            24.0,
        ),
        ("revolved_tube", Box::new(revolve_tube), 20.0),
        (
            "bored_plate",
            Box::new(|m: &mut BRepModel| {
                let plate = sid(TopologyBuilder::new(m)
                    .create_box_3d(50.0, 50.0, 16.0)
                    .expect("plate"));
                let bore = sid(TopologyBuilder::new(m)
                    .create_cylinder_3d(Point3::new(0.0, 0.0, -20.0), Vector3::Z, 10.0, 80.0)
                    .expect("bore"));
                boolean_operation(
                    m,
                    plate,
                    bore,
                    BooleanOp::Difference,
                    BooleanOptions::default(),
                )
                .expect("bore diff")
            }),
            50.0,
        ),
        (
            "boss_union",
            Box::new(|m: &mut BRepModel| {
                let plate = sid(TopologyBuilder::new(m)
                    .create_box_3d(60.0, 60.0, 20.0)
                    .expect("plate"));
                let boss = sid(TopologyBuilder::new(m)
                    .create_cylinder_3d(Point3::new(0.0, 0.0, 5.0), Vector3::Z, 12.0, 20.0)
                    .expect("boss"));
                boolean_operation(m, plate, boss, BooleanOp::Union, BooleanOptions::default())
                    .expect("boss union")
            }),
            60.0,
        ),
    ];

    for (name, build, size) in cases {
        let mut m = BRepModel::new();
        let s = build(&mut m);
        let (brep, mesh) = oracles(&m, s, size);
        assert!(
            brep && mesh,
            "{name}: oracles must AGREE good (brep_valid={brep}, mesh_watertight={mesh}) — a divergence is a perception blind spot"
        );
    }
}

#[test]
fn a_real_defect_is_caught_by_both_oracles() {
    // Remove a face from a valid box's shell → a genuine hole. BOTH oracles must
    // flag it; if either still calls it "good", that oracle is blind.
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("box"));
    // sanity: starts sound
    let (b0, w0) = oracles(&m, s, 20.0);
    assert!(b0 && w0, "box should start sound");

    // Punch a hole: drop one face from the outer shell.
    let shell_id = m.solids.get(s).expect("solid").outer_shell;
    {
        let shell = m.shells.get_mut(shell_id).expect("shell");
        assert!(!shell.faces.is_empty());
        shell.faces.pop();
    }

    let (brep, mesh) = oracles(&m, s, 20.0);
    assert!(!brep, "B-Rep validator must catch the missing face");
    assert!(
        !mesh,
        "mesh oracle must catch the missing face (open edges)"
    );
}

#[test]
fn tessellation_is_deterministic() {
    // The eye must not flicker: the same solid tessellates byte-stable.
    let mut m = BRepModel::new();
    let s = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::ZERO, Vector3::Z, 12.0, 25.0)
        .expect("cyl"));
    let solid = m.solids.get(s).expect("solid");
    let p = TessellationParams::default();
    let a = tessellate_solid(solid, &m, &p);
    let b = tessellate_solid(solid, &m, &p);
    assert_eq!(a.vertices.len(), b.vertices.len(), "vertex count stable");
    assert_eq!(
        a.triangles.len(),
        b.triangles.len(),
        "triangle count stable"
    );
    for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
        assert_eq!(va.position, vb.position, "vertex positions stable");
    }
}
