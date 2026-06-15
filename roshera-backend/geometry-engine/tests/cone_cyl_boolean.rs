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

/// GATE (BOOL #7): a cone fully INSIDE the cylinder (no coincident faces) →
/// cyl∖cone is a clean conical VOID (valid 2-shell solid). cyl(r5,z[0,10]) ∖
/// cone(base r3 @ z=2, apex z=8) → vol = 785.40 − 56.55 = 728.9. This confirms
/// the enclosed-void path works for cones (an earlier "enclosed" pin actually had
/// the cone base COINCIDENT with the cylinder base — a coplanar coincidence,
/// pinned separately below, not an enclosed-void bug).
#[test]
fn cyl_minus_cone_enclosed_void_7() {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, 2.0), Vector3::Z, 3.0, 0.0, 6.0)
        .expect("cone"));
    let res = boolean_operation(
        &mut m,
        cyl,
        cone,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("enclosed cyl∖cone must succeed");
    let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    let inner = m.solids.get(res).map(|s| s.inner_shells.len()).unwrap_or(0);
    eprintln!(
        "[cyl∖cone-enclosed] open={} nm={} valid={} vol={vol:.2} inner_shells={inner}",
        rep.boundary_edges, rep.nonmanifold_edges, v.is_valid
    );
    assert_eq!(
        (rep.boundary_edges, rep.nonmanifold_edges),
        (0, 0),
        "not watertight"
    );
    assert!(v.is_valid, "invalid: {:?}", v.errors);
    let truth = std::f64::consts::PI * 25.0 * 10.0 - std::f64::consts::PI * 9.0 * 6.0 / 3.0;
    assert!(
        (vol - truth).abs() / truth < 0.03,
        "vol {vol:.2} vs truth {truth:.2}"
    );
    assert_eq!(inner, 1, "enclosed cone must form exactly one void shell");
}

/// PIN (BOOL #7 / #32 family): a cone whose BASE is COINCIDENT/coplanar with the
/// cylinder's base cap (both at z=0) — cyl∖cone should be a conical pit opening
/// at the base (vol 785.40 − 94.25 = 691.2). PROGRESS (49f8703-follow-up,
/// get_face_interior_point fix): the cone lateral now classifies Inside (its
/// interior probe is nudged off the coincident base plane) so the cone IS
/// subtracted — VOLUME is now correct (691.0). REMAINING (still invalid): the
/// conical wall isn't WELDED to the base annulus at the coincident rim →
/// boundary-edge gaps (open B-Rep), and tessellation cdt-panics on the unwelded
/// result. So this asserts B-Rep validity (mesh-INDEPENDENT — avoids the
/// tessellation panic) + correct volume; the validity still FAILS (rim weld is
/// the next layer). #32 Same-Domain coincident-face for a cone base. Flip on when
/// the rim weld lands.
#[test]
#[ignore = "#7/#32: cone base coincident with cyl cap — vol now correct, rim weld remains — flip when fixed"]
fn cyl_minus_cone_coincident_base_7() {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 3.0, 0.0, 10.0)
        .expect("cone"));
    let res = boolean_operation(
        &mut m,
        cyl,
        cone,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("coincident-base cyl∖cone must return");
    // Mesh-INDEPENDENT validity (manifold_report tessellates → cdt-panics on the
    // unwelded result; validate_solid_scoped reports the boundary-edge gaps).
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    let truth = std::f64::consts::PI * 25.0 * 10.0 - std::f64::consts::PI * 9.0 * 10.0 / 3.0;
    assert!(
        (vol - truth).abs() / truth < 0.03,
        "vol {vol:.2} vs truth {truth:.2}"
    );
    assert!(v.is_valid, "invalid (rim weld remains): {:?}", v.errors);
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
