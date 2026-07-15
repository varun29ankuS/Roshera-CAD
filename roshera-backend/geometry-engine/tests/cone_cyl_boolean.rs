// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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

/// GATE (BOOL #27/#32 cone family): two FRUSTUMS meeting at a coincident throat
/// rim — a de Laval nozzle (convergent r6→r20 ∪ divergent r18→r6, sharing the
/// r6 throat circle at z=-12). Surfaced building a rocket engine via the API:
/// the union was watertight (open=0) with EXACT volume (15808) but B-Rep INVALID
/// — odd Euler V−E+F−R because the throat was TWO unmerged closed-circle edges.
/// Unlike the cone-on-cylinder "rocket" (closed-circle vs arcs, healed by a
/// T-junction split), here BOTH rims are FULL closed circles sharing the same
/// seam vertex, so there is no foreign vertex to split — and
/// `canonicalise_face_edges_by_position` USED to skip every closed-circle edge
/// (`cs == ce`). FIXED 2026-06-15: canonicalise now welds genuine coincident
/// closed-circle edges (discriminated by the circle's antipode midpoint).
#[test]
fn frustum_union_frustum_throat_27() {
    let mut m = BRepModel::new();
    // convergent: r6 @ z=-12 (throat) → r20 @ z=0
    let conv = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, -12.0), Vector3::Z, 6.0, 20.0, 12.0)
        .expect("convergent frustum"));
    // divergent: r18 @ z=-30 (exit) → r6 @ z=-12 (throat, coincident with conv)
    let div = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, -30.0), Vector3::Z, 18.0, 6.0, 18.0)
        .expect("divergent frustum"));
    let res = boolean_operation(
        &mut m,
        conv,
        div,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("nozzle union must succeed");
    let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    eprintln!(
        "[nozzle] open={} nm={} valid={} vol={vol:.1} (truth ~15808)",
        rep.boundary_edges, rep.nonmanifold_edges, v.is_valid
    );
    assert_eq!(
        (rep.boundary_edges, rep.nonmanifold_edges),
        (0, 0),
        "nozzle not watertight"
    );
    assert!(v.is_valid, "nozzle invalid: {:?}", v.errors);
    // π·h/3·(R²+Rr+r²): conv (r6→r20,h12)=6987.9 + div (r6→r18,h18)=8821.1
    let truth = std::f64::consts::PI * 12.0 / 3.0 * (400.0 + 120.0 + 36.0)
        + std::f64::consts::PI * 18.0 / 3.0 * (324.0 + 108.0 + 36.0);
    assert!(
        (vol - truth).abs() / truth < 0.02,
        "nozzle vol {vol:.1} vs truth {truth:.1}"
    );
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

/// GATE (BOOL #7 / #32 family): a cone whose BASE is COINCIDENT/coplanar with the
/// cylinder's base cap (both at z=0) — cyl∖cone is a conical pit opening at the
/// base (vol 785.40 − 94.25 = 691.2). HISTORY: the cone lateral first failed to
/// be subtracted, then (get_face_interior_point fix) volume became correct but
/// the conical wall stayed UNWELDED at the coincident rim (open B-Rep). FIXED
/// 2026-06-15 by the cone rim-seam alignment fix (see cyl_union_cone_stacked_
/// rocket_27): the rim's seam vertex now matches its curve `t = 0`, so the
/// T-junction healer can split + weld the coincident base rim. Asserts B-Rep
/// validity (mesh-INDEPENDENT — the fine tessellation of the conical pit can still
/// cdt-stress, but the B-Rep is sound) + correct volume.
#[test]
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

/// GATE (BOOL #27/#32 cone family — the "rocket"): a cone stacked coaxially on
/// the cylinder's top cap (cone base circle r5 @ z=10 coincident with the cyl top
/// rim) ∪ → a rocket (cylinder body + cone nose). Volume was always correct
/// (cyl 785.40 + cone 130.90 = 916.30), so it RENDERED right, but the result was
/// a hollow husk: 279 open edges, invalid.
///
/// ROOT CAUSE (fixed 2026-06-15): the cone primitive placed its rim seam VERTEX
/// at `center + axis.perpendicular()·r` (e.g. +Y for a −Z internal axis) while
/// the rim `Circle` curve's `t = 0` sits at the canonical +X. The full-cone rim
/// edge's `param_range` of [0,1] therefore did NOT start at its `start_vertex`,
/// so `heal_t_junctions_across_faces` saw the coincident foreign vertex land on
/// the param boundary (t = 0) and could not split the closed circle — the rim
/// stayed welded on the cylinder side only. Fix: derive the cone `ref_dir` from
/// `Circle::x_axis()` so surface seam, rim curve `t = 0`, seam vertex, and edge
/// `param_range` all coincide (cone_primitive.rs). A periodic-wrap guard was also
/// added to the T-junction healer (boolean.rs) as defence in depth.
#[test]
fn cyl_union_cone_stacked_rocket_27() {
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 10.0)
        .expect("cyl"));
    // Cone base (r5) sits exactly on the cylinder top cap (z=10); nose at z=15.
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, 10.0), Vector3::Z, 5.0, 0.0, 5.0)
        .expect("cone"));
    let res = boolean_operation(
        &mut m,
        cyl,
        cone,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("stacked rocket union must succeed");
    let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    eprintln!(
        "[rocket] open={} nm={} valid={} vol={vol:.2} (truth 916.30)",
        rep.boundary_edges, rep.nonmanifold_edges, v.is_valid
    );
    assert_eq!(
        (rep.boundary_edges, rep.nonmanifold_edges),
        (0, 0),
        "stacked rocket not watertight"
    );
    assert!(v.is_valid, "stacked rocket invalid: {:?}", v.errors);
    let truth = std::f64::consts::PI * 25.0 * 10.0 + std::f64::consts::PI * 25.0 * 5.0 / 3.0;
    assert!(
        (vol - truth).abs() / truth < 0.02,
        "rocket vol {vol:.2} vs truth {truth:.2}"
    );
}
