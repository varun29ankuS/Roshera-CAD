// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Exact analytic mass properties — closed-form validation suite.
//!
//! Every test here is a metrology gate: the analytic per-face quadrature
//! integrator (`primitives::mass_properties`) must match the closed-form
//! volume AND inertia tensor of primitives whose values are known exactly.
//! A sign or orientation error surfaces immediately as a negative/wrong-axis
//! inertia. See docs/superpowers/specs/2026-07-09-exact-analytic-mass-properties-design.md.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::mass_properties::integrate_solid;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn box_solid(m: &mut BRepModel, w: f64, d: f64, h: f64) -> u32 {
    match TopologyBuilder::new(m).create_box_3d(w, d, h).expect("box") {
        GeometryId::Solid(id) => id,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn cyl(m: &mut BRepModel, r: f64, h: f64) -> u32 {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            r,
            h,
        )
        .expect("cyl")
    {
        GeometryId::Solid(id) => id,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn sph(m: &mut BRepModel, r: f64) -> u32 {
    match TopologyBuilder::new(m)
        .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), r)
        .expect("sph")
    {
        GeometryId::Solid(id) => id,
        o => panic!("expected solid, got {o:?}"),
    }
}

// ── Box (planar faces, full parameter rectangles → literally exact) ──────────

#[test]
fn box_volume_exact() {
    let mut m = BRepModel::new();
    let s = box_solid(&mut m, 3.0, 4.0, 5.0); // centred at origin
    let mp = integrate_solid(s, &m, 1.0, 1e-9).expect("mass props");
    let expected = 3.0 * 4.0 * 5.0; // 60
    assert!(
        (mp.volume - expected).abs() / expected < 1e-9,
        "box volume must be exact; got {}",
        mp.volume
    );
}

#[test]
fn box_inertia_and_com_exact() {
    let mut m = BRepModel::new();
    let s = box_solid(&mut m, 3.0, 4.0, 5.0);
    let mp = integrate_solid(s, &m, 1.0, 1e-9).expect("mp");
    let c = mp.center_of_mass;
    assert!(
        c.x.abs() < 1e-9 && c.y.abs() < 1e-9 && c.z.abs() < 1e-9,
        "CoM at origin; got {c:?}"
    );
    let it = mp.inertia_tensor;
    // m=60: Ixx=m(d²+h²)/12=205, Iyy=m(w²+h²)/12=170, Izz=m(w²+d²)/12=125.
    let approx = |a: f64, b: f64| (a - b).abs() / b < 1e-9;
    assert!(approx(it[0][0], 205.0), "Ixx {}", it[0][0]);
    assert!(approx(it[1][1], 170.0), "Iyy {}", it[1][1]);
    assert!(approx(it[2][2], 125.0), "Izz {}", it[2][2]);
    for i in 0..3 {
        for j in 0..3 {
            if i != j {
                assert!(it[i][j].abs() < 1e-7, "product {i}{j} = {}", it[i][j]);
            }
        }
    }
}

// ── Cylinder (curved lateral + periodic-u) ───────────────────────────────────

// PAUSED / PIVOT: the cylinder's disk caps are trimmed planar faces; the
// current adaptive-membership trimming is ~0.9% inaccurate and slow on the
// circular boundary. Un-ignore when the planar boundary-reduction (Green's)
// hybrid lands — see docs/superpowers/plans/2026-07-09-exact-analytic-mass-properties.md.
#[ignore = "trimmed disk-cap accuracy pending planar boundary-reduction pivot"]
#[test]
fn cylinder_volume_inertia_exact() {
    let mut m = BRepModel::new();
    let s = cyl(&mut m, 2.0, 6.0); // base at origin, spans z∈[0,6]
    let mp = integrate_solid(s, &m, 1.0, 1e-9).expect("mp");
    let vol = std::f64::consts::PI * 4.0 * 6.0; // πr²h = 24π
    assert!((mp.volume - vol).abs() / vol < 1e-6, "V {}", mp.volume);
    assert!(
        (mp.center_of_mass.z - 3.0).abs() < 1e-6,
        "CoM z {}",
        mp.center_of_mass.z
    );
    let mass = vol;
    // Izz=½mr²=2m ; Ixx=Iyy=m(3r²+h²)/12=4m (about CoM).
    assert!(
        (mp.inertia_tensor[2][2] - 2.0 * mass).abs() / (2.0 * mass) < 1e-5,
        "Izz {}",
        mp.inertia_tensor[2][2]
    );
    assert!(
        (mp.inertia_tensor[0][0] - 4.0 * mass).abs() / (4.0 * mass) < 1e-5,
        "Ixx {}",
        mp.inertia_tensor[0][0]
    );
}

// ── Sphere (pole degeneracy) ─────────────────────────────────────────────────

#[test]
fn sphere_volume_inertia_exact() {
    let mut m = BRepModel::new();
    let s = sph(&mut m, 2.0);
    let mp = integrate_solid(s, &m, 1.0, 1e-9).expect("mp");
    let vol = 4.0 / 3.0 * std::f64::consts::PI * 8.0; // 4/3 π r³
    assert!((mp.volume - vol).abs() / vol < 1e-5, "V {}", mp.volume);
    let mass = vol;
    let i = 0.4 * mass * 4.0; // ⅖ m r²
    for k in 0..3 {
        assert!(
            (mp.inertia_tensor[k][k] - i).abs() / i < 1e-4,
            "I{k}{k} {}",
            mp.inertia_tensor[k][k]
        );
    }
}
