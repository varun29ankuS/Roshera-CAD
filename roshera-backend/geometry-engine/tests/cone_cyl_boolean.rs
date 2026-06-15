//! CONE∘CYLINDER boolean characterization (task #7 — analytic cyl∘cone SSI).
//! Currently routes to the step-capped marcher (no analytic arm) → terminates
//! but geometry is unreliable. Coaxial cases here; both on the +Z axis.
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

fn sid(g: GeometryId) -> geometry_engine::primitives::solid::SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("{o:?}"),
    }
}

/// GATE (BOOL #7): a coaxial cone WIDER than the cylinder pokes transversally
/// through the wall — the analytic cone×cylinder arm cuts it in one circle where
/// the cone radius equals rc. cyl(r5,z[0,10]) ∖ cone(base r8 at z=0, apex z=10)
/// must be watertight + valid (was 100 open / invalid via marching).
#[test]
fn cyl_minus_cone_transverse_7() {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 8.0, 0.0, 10.0)
        .expect("cone"));
    let res = boolean_operation(
        &mut m,
        cyl,
        cone,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("transverse cyl∖cone must succeed");
    let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    eprintln!(
        "[cyl∖cone-transverse] open={} nm={} valid={}",
        rep.boundary_edges, rep.nonmanifold_edges, v.is_valid
    );
    assert_eq!(
        (rep.boundary_edges, rep.nonmanifold_edges),
        (0, 0),
        "transverse cyl∖cone not watertight"
    );
    assert!(v.is_valid, "transverse cyl∖cone invalid: {:?}", v.errors);
}

#[test]
#[ignore = "task #7 characterization — run with --ignored --nocapture"]
fn diag_cone_cyl_current_state() {
    // cylinder r5, z[0,10], +Z.
    let cases: [(&str, f64, f64, f64, f64, BooleanOp); 4] = [
        // (label, cone base_r, top_r, base_z, height, op)
        (
            "cyl∖cone-enclosed",
            3.0,
            0.0,
            0.0,
            10.0,
            BooleanOp::Difference,
        ),
        ("cyl∪cone-enclosed", 3.0, 0.0, 0.0, 10.0, BooleanOp::Union),
        (
            "cyl∖cone-transverse",
            8.0,
            0.0,
            0.0,
            10.0,
            BooleanOp::Difference,
        ),
        ("cyl∪cone-stacked", 5.0, 0.0, 10.0, 5.0, BooleanOp::Union), // cone atop, base=rim
    ];
    for (label, br, tr, bz, h, op) in cases {
        let mut m = BRepModel::new();
        let cyl = sid(TopologyBuilder::new(&mut m)
            .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
            .expect("cyl"));
        let cone = sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::new(0.0, 0.0, bz), Vector3::Z, br, tr, h)
            .expect("cone"));
        match boolean_operation(&mut m, cyl, cone, op, BooleanOptions::default()) {
            Ok(res) => {
                let rep = manifold_report(&m, res, 0.5, 1e-6);
                let (open, nm) = rep
                    .map(|r| (r.boundary_edges, r.nonmanifold_edges))
                    .unwrap_or((9999, 9999));
                let v =
                    validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
                let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
                eprintln!(
                    "[conecyl] {label}: open={open} nm={nm} valid={} vol={vol:.2}",
                    v.is_valid
                );
            }
            Err(e) => eprintln!("[conecyl] {label}: ERROR {e:?}"),
        }
    }
}
