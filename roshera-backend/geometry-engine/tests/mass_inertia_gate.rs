//! Analytic inertia-tensor gate (NON-ignored).
//!
//! The 2026-06-11 Parasolid-parity audit flagged "inertia tensor = bounding
//! box approximation". Investigation showed the SERVED path is already
//! correct: `compute_solid_mass_properties` always routes through
//! `mesh_based_mass_properties`, which integrates true second moments per
//! tetrahedron (Tonon 2004); the bbox approximation lives only in the
//! deliberately-bypassed `Shell::compute_mass_properties` analytical path
//! (documented at the dispatch site, topology_builder.rs). What was missing
//! is an ACCURACY gate: `primitive_mass_invariants.rs` asserts the tensor
//! is finite, never that it is right — a bbox-approximate tensor would have
//! passed every existing test. This gate pins inertia/mass ratios against
//! closed form for centered primitives (origin frame == COM frame):
//!   box w×h×d:    I/m = diag((h²+d²), (w²+d²), (w²+h²)) / 12
//!   sphere r:     I/m = diag(2r²/5)
//!   cylinder r,h: I/m = diag((3r²+h²)/12, (3r²+h²)/12, r²/2)   (axis = Z)
//! Off-diagonals must vanish (within mesh noise) for these symmetric
//! solids. Tolerances follow the file-documented mesh ceilings: 3% box,
//! 5% curved.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

#[allow(clippy::expect_used, clippy::panic)] // test fixture
fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid geometry, got {other:?}"),
    }
}

fn assert_inertia(
    label: &str,
    model: &mut BRepModel,
    id: SolidId,
    expected_diag_over_m: [f64; 3],
    rel_tol: f64,
) {
    #[allow(clippy::expect_used)] // gate fixture: construction verified above
    let r = model
        .mass_properties_for(id)
        .expect("mass properties report");
    assert!(
        r.mass > 0.0 && r.mass.is_finite(),
        "[{label}] mass {}",
        r.mass
    );
    let max_diag = expected_diag_over_m.iter().fold(0.0_f64, |a, &b| a.max(b));
    for i in 0..3 {
        let got = r.inertia_tensor[i][i] / r.mass;
        let want = expected_diag_over_m[i];
        let rel = (got - want).abs() / want;
        assert!(
            rel <= rel_tol,
            "[{label}] I[{i}][{i}]/m = {got:.5} vs analytic {want:.5} (rel {rel:.4})"
        );
        for j in 0..3 {
            if i == j {
                continue;
            }
            let off = r.inertia_tensor[i][j].abs() / r.mass;
            assert!(
                off <= max_diag * rel_tol,
                "[{label}] off-diagonal I[{i}][{j}]/m = {off:.6} exceeds noise bound"
            );
        }
    }
}

/// Centered box 2×2×2: I/m = (4+4)/12 = 2/3 on every axis.
#[test]
fn box_inertia_matches_analytic() {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        #[allow(clippy::expect_used)]
        expect_solid(b.create_box_3d(2.0, 2.0, 2.0).expect("box"))
    };
    assert_inertia("box 2x2x2", &mut model, id, [2.0 / 3.0; 3], 0.03);
}

/// Asymmetric centered box 2×1×0.5 — catches axis mix-ups a cube cannot:
/// I/m = ((1+0.25), (4+0.25), (4+1)) / 12.
#[test]
fn asymmetric_box_inertia_matches_analytic() {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        #[allow(clippy::expect_used)]
        expect_solid(b.create_box_3d(2.0, 1.0, 0.5).expect("box"))
    };
    assert_inertia(
        "box 2x1x0.5",
        &mut model,
        id,
        [1.25 / 12.0, 4.25 / 12.0, 5.0 / 12.0],
        0.03,
    );
}

/// Unit sphere at origin: I/m = 2/5 on every axis.
#[test]
fn sphere_inertia_matches_analytic() {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        #[allow(clippy::expect_used)]
        expect_solid(b.create_sphere_3d(Point3::ORIGIN, 1.0).expect("sphere"))
    };
    assert_inertia("sphere r=1", &mut model, id, [0.4; 3], 0.05);
}

/// Centered Z-axis cylinder r=0.5 h=2 (base at z=−1):
/// Ixx/m = Iyy/m = (3r²+h²)/12 = 4.75/12, Izz/m = r²/2 = 0.125.
#[test]
fn cylinder_inertia_matches_analytic() {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        #[allow(clippy::expect_used)]
        expect_solid(
            b.create_cylinder_3d(Point3::new(0.0, 0.0, -1.0), Vector3::Z, 0.5, 2.0)
                .expect("cylinder"),
        )
    };
    assert_inertia(
        "cylinder r=0.5 h=2",
        &mut model,
        id,
        [4.75 / 12.0, 4.75 / 12.0, 0.125],
        0.05,
    );
}
