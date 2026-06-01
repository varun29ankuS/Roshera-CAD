//! Inertia-tensor invariants for the mass-properties pipeline.
//!
//! The inertia tensor of any rigid solid is symmetric positive-definite, so
//! its principal moments are positive and finite. Symmetry of the solid
//! constrains them further: a sphere is isotropic (all three equal), a
//! cylinder/right prism has two equal radial moments. And because inertia is
//! mass × length², uniformly scaling a solid by s scales every principal
//! moment by s⁵ (mass ∝ s³, length² ∝ s²).

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn expect_solid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn box_moments(w: f64, h: f64, d: f64) -> ([[f64; 3]; 3], [f64; 3], f64) {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        expect_solid(b.create_box_3d(w, h, d).expect("box"))
    };
    let mp = model.mass_properties_for(id).expect("mass props");
    let pm = [
        mp.principal_moments[0],
        mp.principal_moments[1],
        mp.principal_moments[2],
    ];
    (mp.inertia_tensor, pm, mp.volume)
}

fn sphere_moments(r: f64) -> [f64; 3] {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        expect_solid(b.create_sphere_3d(Point3::ORIGIN, r).expect("sphere"))
    };
    let mp = model.mass_properties_for(id).expect("mp");
    [
        mp.principal_moments[0],
        mp.principal_moments[1],
        mp.principal_moments[2],
    ]
}

fn cylinder_moments(r: f64, h: f64) -> [f64; 3] {
    let mut model = BRepModel::new();
    let id = {
        let mut b = TopologyBuilder::new(&mut model);
        expect_solid(
            b.create_cylinder_3d(Point3::ORIGIN, Vector3::Z, r, h)
                .expect("cyl"),
        )
    };
    let mp = model.mass_properties_for(id).expect("mp");
    [
        mp.principal_moments[0],
        mp.principal_moments[1],
        mp.principal_moments[2],
    ]
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b == 0.0 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

// =====================================================================
// Symmetric positive-definite tensor.
// =====================================================================

macro_rules! inertia_spd_test {
    ($name:ident, $w:expr, $h:expr, $d:expr) => {
        #[test]
        fn $name() {
            let (it, pm, _vol) = box_moments($w, $h, $d);
            let scale = pm[0].abs().max(pm[1].abs()).max(pm[2].abs()).max(1.0);
            // Symmetric.
            for i in 0..3 {
                for j in 0..3 {
                    assert!(it[i][j].is_finite(), "non-finite inertia[{i}][{j}]");
                    assert!(
                        (it[i][j] - it[j][i]).abs() <= 1e-3 * scale,
                        "inertia not symmetric at [{i}][{j}]: {} vs {}",
                        it[i][j],
                        it[j][i]
                    );
                }
            }
            // Principal moments positive and finite.
            for (k, &m) in pm.iter().enumerate() {
                assert!(
                    m > 0.0 && m.is_finite(),
                    "principal moment {k} = {m} not positive-finite"
                );
            }
            // Triangle inequality on principal moments (a real rigid body):
            // each moment ≤ sum of the other two.
            assert!(pm[0] <= pm[1] + pm[2] + 1e-3 * scale, "I0 > I1+I2");
            assert!(pm[1] <= pm[0] + pm[2] + 1e-3 * scale, "I1 > I0+I2");
            assert!(pm[2] <= pm[0] + pm[1] + 1e-3 * scale, "I2 > I0+I1");
        }
    };
}

inertia_spd_test!(spd_cube, 2.0, 2.0, 2.0);
inertia_spd_test!(spd_2_3_4, 2.0, 3.0, 4.0);
inertia_spd_test!(spd_slab, 8.0, 1.0, 6.0);
inertia_spd_test!(spd_tall, 1.0, 10.0, 1.0);
inertia_spd_test!(spd_5_5_2, 5.0, 5.0, 2.0);

// =====================================================================
// Sphere isotropy: all three principal moments equal.
// =====================================================================

macro_rules! sphere_isotropy_test {
    ($name:ident, $r:expr) => {
        #[test]
        fn $name() {
            let m = sphere_moments($r);
            let avg = (m[0] + m[1] + m[2]) / 3.0;
            for (k, &mk) in m.iter().enumerate() {
                assert!(
                    rel_close(mk, avg, 0.05),
                    "sphere principal moment {k}={mk} differs from mean {avg} (not isotropic)"
                );
            }
        }
    };
}

sphere_isotropy_test!(sphere_iso_1, 1.0);
sphere_isotropy_test!(sphere_iso_3, 3.0);
sphere_isotropy_test!(sphere_iso_5, 5.0);
sphere_isotropy_test!(sphere_iso_2p5, 2.5);

// =====================================================================
// Cylinder: exactly two principal moments equal (radial symmetry).
// =====================================================================

macro_rules! cylinder_symmetry_test {
    ($name:ident, $r:expr, $h:expr) => {
        #[test]
        fn $name() {
            let mut m = cylinder_moments($r, $h);
            m.sort_by(|a, b| a.partial_cmp(b).unwrap());
            // Among the three sorted moments, one adjacent pair must be
            // (nearly) equal — the two radial axes of the right cylinder.
            let pair_lo = rel_close(m[0], m[1], 0.04);
            let pair_hi = rel_close(m[1], m[2], 0.04);
            assert!(
                pair_lo || pair_hi,
                "cylinder r={} h={} has no equal moment pair: {:?}",
                $r,
                $h,
                m
            );
        }
    };
}

cylinder_symmetry_test!(cyl_sym_2_6, 2.0, 6.0);
cylinder_symmetry_test!(cyl_sym_1_10, 1.0, 10.0);
cylinder_symmetry_test!(cyl_sym_3_3, 3.0, 3.0);
cylinder_symmetry_test!(cyl_sym_5_2, 5.0, 2.0);

// =====================================================================
// Uniform scaling multiplies principal moments by s⁵.
// =====================================================================

#[test]
fn inertia_scales_as_fifth_power_of_linear_size() {
    // Two cubes, edge 2 and edge 4 (factor s = 2). Each principal moment of
    // the larger must be 2⁵ = 32× the smaller's (same density).
    let (_, small, _) = box_moments(2.0, 2.0, 2.0);
    let (_, large, _) = box_moments(4.0, 4.0, 4.0);
    let mut s = small;
    let mut l = large;
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    l.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for k in 0..3 {
        let ratio = l[k] / s[k];
        assert!(
            rel_close(ratio, 32.0, 0.05),
            "principal moment {k} ratio {ratio} != 32 (s⁵ for s=2)",
        );
    }
}

#[test]
fn inertia_scales_as_fifth_power_factor_three() {
    // s = 3 ⇒ 3⁵ = 243.
    let (_, small, _) = box_moments(1.0, 2.0, 3.0);
    let (_, large, _) = box_moments(3.0, 6.0, 9.0);
    let mut s = small;
    let mut l = large;
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    l.sort_by(|a, b| a.partial_cmp(b).unwrap());
    for k in 0..3 {
        let ratio = l[k] / s[k];
        assert!(
            rel_close(ratio, 243.0, 0.05),
            "moment {k} ratio {ratio} != 243"
        );
    }
}

#[test]
fn elongated_box_has_smallest_moment_about_long_axis() {
    // A long thin box (long in y) has its smallest principal moment about the
    // long axis. The smallest moment should be well below the largest.
    let (_, pm, _) = box_moments(1.0, 12.0, 1.0);
    let mut m = pm;
    m.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!(m[0] < m[2], "elongated box moments should differ: {m:?}");
    assert!(m[0] > 0.0, "smallest moment must be positive");
}
