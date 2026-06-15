//! CONE∘SPHERE boolean characterization (task #7 — analytic cone∘sphere SSI).
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

/// GATE (BOOL #7): coaxial cone∘sphere cases the analytic arm makes CORRECT.
/// MC-verified truths: cone∖sphere(r6@z0)=19.50, cone∪sphere(r3@z8)=350.11.
/// (cone: base r5 @ z=0, apex z=10, +Z.)
#[test]
fn cone_sphere_correct_cases_7() {
    let build = |sz: f64, sr: f64| {
        let mut m = BRepModel::new();
        let cone = sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 0.0, 10.0)
            .expect("cone"));
        let sph = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::new(0.0, 0.0, sz), sr)
            .expect("sphere"));
        (m, cone, sph)
    };
    let check = |label: &str, sz: f64, sr: f64, op: BooleanOp, truth: f64| {
        let (mut m, cone, sph) = build(sz, sr);
        let res = boolean_operation(&mut m, cone, sph, op, BooleanOptions::default())
            .unwrap_or_else(|e| panic!("{label}: {e:?}"));
        let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
        let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
        let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
        eprintln!(
            "[{label}] open={} nm={} valid={} vol={vol:.2} (truth {truth:.2})",
            rep.boundary_edges, rep.nonmanifold_edges, v.is_valid
        );
        assert_eq!(
            (rep.boundary_edges, rep.nonmanifold_edges),
            (0, 0),
            "{label} not watertight"
        );
        assert!(v.is_valid, "{label} invalid: {:?}", v.errors);
        assert!(
            (vol - truth).abs() / truth < 0.03,
            "{label} vol {vol:.2} vs truth {truth:.2}"
        );
    };
    check(
        "cone∖sphere-transverse",
        0.0,
        6.0,
        BooleanOp::Difference,
        19.50,
    );
    check("cone∪sphere-tip", 8.0, 3.0, BooleanOp::Union, 350.11);
}

/// PIN (BOOL #7): cone∪sphere where the sphere transversally engulfs the lower
/// cone — the analytic SSI circle is correct (the matching DIFFERENCE is exact,
/// see the gate above) but the UNION region-selection over-reports (~2×): MC
/// truth 924.11, kernel ~1602, invalid. A downstream union classify/select bug,
/// not the SSI. Flip on when fixed.
#[test]
#[ignore = "#7: cone∪sphere transverse union over-inclusion (downstream classify) — flip when fixed"]
fn cone_union_sphere_transverse_overinclusion_7() {
    let mut m = BRepModel::new();
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 0.0, 10.0)
        .expect("cone"));
    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 6.0)
        .expect("sphere"));
    let res = boolean_operation(
        &mut m,
        cone,
        sph,
        BooleanOp::Union,
        BooleanOptions::default(),
    )
    .expect("union");
    let rep = manifold_report(&m, res, 0.5, 1e-6).expect("mesh");
    let v = validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
    let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
    assert_eq!((rep.boundary_edges, rep.nonmanifold_edges), (0, 0));
    assert!(v.is_valid, "invalid: {:?}", v.errors);
    assert!(
        (vol - 924.11).abs() / 924.11 < 0.03,
        "vol {vol:.2} vs truth 924.11"
    );
}

#[test]
#[ignore = "characterization — run with --ignored --nocapture"]
fn diag_cone_sphere_current_state() {
    // cone: base r5 at z=0, apex z=10 (+Z), coaxial sphere at (0,0,sz) radius sr.
    let cases: [(&str, f64, f64, BooleanOp); 4] = [
        ("cone∖sphere-transverse", 0.0, 6.0, BooleanOp::Difference),
        ("cone∪sphere-transverse", 0.0, 6.0, BooleanOp::Union),
        ("cone∖sphere-enclosed", 2.5, 1.5, BooleanOp::Difference),
        ("cone∪sphere-tip", 8.0, 3.0, BooleanOp::Union),
    ];
    for (label, sz, sr, op) in cases {
        let mut m = BRepModel::new();
        let cone = sid(TopologyBuilder::new(&mut m)
            .create_cone_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 5.0, 0.0, 10.0)
            .expect("cone"));
        let sph = sid(TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::new(0.0, 0.0, sz), sr)
            .expect("sphere"));
        match boolean_operation(&mut m, cone, sph, op, BooleanOptions::default()) {
            Ok(res) => {
                let rep = manifold_report(&m, res, 0.5, 1e-6);
                let (open, nm) = rep
                    .map(|r| (r.boundary_edges, r.nonmanifold_edges))
                    .unwrap_or((9999, 9999));
                let v =
                    validate_solid_scoped(&m, res, Tolerance::default(), ValidationLevel::Standard);
                let vol = m.calculate_solid_volume(res).unwrap_or(f64::NAN);
                eprintln!(
                    "[conesph] {label}: open={open} nm={nm} valid={} vol={vol:.2}",
                    v.is_valid
                );
            }
            Err(e) => eprintln!("[conesph] {label}: ERROR {e:?}"),
        }
    }
}
