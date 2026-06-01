//! Mass-property invariants for the analytic primitives.
//!
//! Every analytic primitive (box, sphere, cylinder, cone, frustum) has a
//! closed-form volume and surface area. These tests pin the kernel's
//! computed `MassPropertiesReport` against those formulas across a wide
//! table of dimensions, plus structural sanity (positive, finite volume /
//! area / mass and a finite centre of mass) on every case.
//!
//! Tolerances: the volume and curved-surface-area numbers come from the
//! tessellated divergence-theorem pipeline (`Solid::compute_mass_properties`
//! is the source of truth), so a curved primitive carries mesh-driven error.
//! We use 5–6 % relative tolerance on curved volumes — the same ceiling
//! `kernel_workflow_regression.rs` documents — and a tight 3 % on the box,
//! whose faces are planar and integrate near-exactly. Surface-area formulas
//! are asserted exactly only for the box; curved areas are checked for
//! positivity and finiteness (coarse tessellation under-counts curved area
//! by more than the volume error, so an exact bound would be brittle).

use std::f64::consts::PI;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use proptest::prelude::*;

// ---------------------------------------------------------------------
// Construction helpers (builder scoped so the `&mut model` borrow is
// released before `mass_properties_for(&mut model)` is called).
// ---------------------------------------------------------------------

fn expect_solid(geom: GeometryId) -> SolidId {
    match geom {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid geometry, got {other:?}"),
    }
}

fn make_box(model: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(builder.create_box_3d(w, h, d).expect("box creation"))
}

fn make_sphere(model: &mut BRepModel, radius: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_sphere_3d(Point3::ORIGIN, radius)
            .expect("sphere creation"),
    )
}

fn make_cylinder(model: &mut BRepModel, radius: f64, height: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
            .expect("cylinder creation"),
    )
}

fn make_cone(model: &mut BRepModel, base_radius: f64, top_radius: f64, height: f64) -> SolidId {
    let mut builder = TopologyBuilder::new(model);
    expect_solid(
        builder
            .create_cone_3d(Point3::ORIGIN, Vector3::Z, base_radius, top_radius, height)
            .expect("cone creation"),
    )
}

fn rel_close(actual: f64, expected: f64, rel_tol: f64) -> bool {
    if expected == 0.0 {
        return actual.abs() <= rel_tol;
    }
    ((actual - expected) / expected).abs() <= rel_tol
}

/// Shared structural sanity asserted on every primitive's report.
macro_rules! assert_report_sane {
    ($r:expr) => {{
        let r = &$r;
        assert!(
            r.volume > 0.0 && r.volume.is_finite(),
            "volume {}",
            r.volume
        );
        assert!(
            r.surface_area > 0.0 && r.surface_area.is_finite(),
            "surface_area {}",
            r.surface_area
        );
        assert!(r.mass > 0.0 && r.mass.is_finite(), "mass {}", r.mass);
        assert!(
            r.center_of_mass.iter().all(|c| c.is_finite()),
            "non-finite COM {:?}",
            r.center_of_mass
        );
        for i in 0..3 {
            for j in 0..3 {
                assert!(
                    r.inertia_tensor[i][j].is_finite(),
                    "non-finite inertia[{i}][{j}]"
                );
            }
        }
    }};
}

// ---------------------------------------------------------------------
// Box: volume = w·h·d, surface area = 2(wh + hd + wd). Planar faces, so
// both integrate near-exactly.
// ---------------------------------------------------------------------

macro_rules! box_volume_test {
    ($name:ident, $w:expr, $h:expr, $d:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let r = model.mass_properties_for(id).expect("box mass props");
            let expected = ($w as f64) * ($h as f64) * ($d as f64);
            assert!(
                rel_close(r.volume, expected, 0.03),
                "box {}x{}x{}: volume {} vs expected {}",
                $w,
                $h,
                $d,
                r.volume,
                expected
            );
            assert_report_sane!(r);
        }
    };
}

box_volume_test!(box_vol_unit_cube, 1.0, 1.0, 1.0);
box_volume_test!(box_vol_2_3_4, 2.0, 3.0, 4.0);
box_volume_test!(box_vol_half, 0.5, 0.5, 0.5);
box_volume_test!(box_vol_10_2_5, 10.0, 2.0, 5.0);
box_volume_test!(box_vol_tall, 1.0, 10.0, 1.0);
box_volume_test!(box_vol_3_3_3, 3.0, 3.0, 3.0);
box_volume_test!(box_vol_5_1_2, 5.0, 1.0, 2.0);
box_volume_test!(box_vol_slab, 2.0, 2.0, 8.0);
box_volume_test!(box_vol_7_4_1, 7.0, 4.0, 1.0);
box_volume_test!(box_vol_fractional, 1.5, 2.5, 3.5);
box_volume_test!(box_vol_big_cube, 20.0, 20.0, 20.0);
box_volume_test!(box_vol_6_6_2, 6.0, 6.0, 2.0);
box_volume_test!(box_vol_4_9_4, 4.0, 9.0, 4.0);
box_volume_test!(box_vol_8_3_7, 8.0, 3.0, 7.0);
box_volume_test!(box_vol_wide_thin, 12.0, 0.5, 9.0);

macro_rules! box_area_test {
    ($name:ident, $w:expr, $h:expr, $d:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let r = model.mass_properties_for(id).expect("box mass props");
            let (w, h, d) = ($w as f64, $h as f64, $d as f64);
            let expected = 2.0 * (w * h + h * d + w * d);
            assert!(
                rel_close(r.surface_area, expected, 0.03),
                "box {}x{}x{}: area {} vs expected {}",
                $w,
                $h,
                $d,
                r.surface_area,
                expected
            );
            assert_report_sane!(r);
        }
    };
}

box_area_test!(box_area_unit_cube, 1.0, 1.0, 1.0);
box_area_test!(box_area_2_3_4, 2.0, 3.0, 4.0);
box_area_test!(box_area_10_2_5, 10.0, 2.0, 5.0);
box_area_test!(box_area_3_3_3, 3.0, 3.0, 3.0);
box_area_test!(box_area_5_1_2, 5.0, 1.0, 2.0);
box_area_test!(box_area_slab, 2.0, 2.0, 8.0);
box_area_test!(box_area_fractional, 1.5, 2.5, 3.5);
box_area_test!(box_area_big_cube, 20.0, 20.0, 20.0);
box_area_test!(box_area_6_6_2, 6.0, 6.0, 2.0);
box_area_test!(box_area_8_3_7, 8.0, 3.0, 7.0);

// ---------------------------------------------------------------------
// Sphere: volume = 4/3·π·r³.
// ---------------------------------------------------------------------

macro_rules! sphere_volume_test {
    ($name:ident, $r:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_sphere(&mut model, $r);
            let report = model.mass_properties_for(id).expect("sphere mass props");
            let r = $r as f64;
            let expected = 4.0 / 3.0 * PI * r * r * r;
            assert!(
                rel_close(report.volume, expected, 0.05),
                "sphere r={}: volume {} vs expected {}",
                $r,
                report.volume,
                expected
            );
            assert_report_sane!(report);
        }
    };
}

sphere_volume_test!(sphere_vol_r_0p5, 0.5);
sphere_volume_test!(sphere_vol_r_1, 1.0);
sphere_volume_test!(sphere_vol_r_2, 2.0);
sphere_volume_test!(sphere_vol_r_3, 3.0);
sphere_volume_test!(sphere_vol_r_5, 5.0);
sphere_volume_test!(sphere_vol_r_0p25, 0.25);
sphere_volume_test!(sphere_vol_r_1p5, 1.5);
sphere_volume_test!(sphere_vol_r_2p5, 2.5);
sphere_volume_test!(sphere_vol_r_4, 4.0);
sphere_volume_test!(sphere_vol_r_7, 7.0);
sphere_volume_test!(sphere_vol_r_10, 10.0);
sphere_volume_test!(sphere_vol_r_0p75, 0.75);
sphere_volume_test!(sphere_vol_r_6, 6.0);
sphere_volume_test!(sphere_vol_r_8, 8.0);
sphere_volume_test!(sphere_vol_r_1p25, 1.25);

// ---------------------------------------------------------------------
// Cylinder: volume = π·r²·h.
// ---------------------------------------------------------------------

macro_rules! cylinder_volume_test {
    ($name:ident, $r:expr, $h:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_cylinder(&mut model, $r, $h);
            let report = model.mass_properties_for(id).expect("cylinder mass props");
            let (r, h) = ($r as f64, $h as f64);
            let expected = PI * r * r * h;
            assert!(
                rel_close(report.volume, expected, 0.05),
                "cylinder r={} h={}: volume {} vs expected {}",
                $r,
                $h,
                report.volume,
                expected
            );
            assert_report_sane!(report);
        }
    };
}

cylinder_volume_test!(cyl_vol_1_1, 1.0, 1.0);
cylinder_volume_test!(cyl_vol_2_5, 2.0, 5.0);
cylinder_volume_test!(cyl_vol_0p5_3, 0.5, 3.0);
cylinder_volume_test!(cyl_vol_3_2, 3.0, 2.0);
cylinder_volume_test!(cyl_vol_5_10, 5.0, 10.0);
cylinder_volume_test!(cyl_vol_1p5_4, 1.5, 4.0);
cylinder_volume_test!(cyl_vol_2p5_2p5, 2.5, 2.5);
cylinder_volume_test!(cyl_vol_4_1, 4.0, 1.0);
cylinder_volume_test!(cyl_vol_0p25_8, 0.25, 8.0);
cylinder_volume_test!(cyl_vol_6_3, 6.0, 3.0);
cylinder_volume_test!(cyl_vol_1_10, 1.0, 10.0);
cylinder_volume_test!(cyl_vol_3_3, 3.0, 3.0);
cylinder_volume_test!(cyl_vol_2_7, 2.0, 7.0);
cylinder_volume_test!(cyl_vol_5_5, 5.0, 5.0);
cylinder_volume_test!(cyl_vol_0p75_6, 0.75, 6.0);

// ---------------------------------------------------------------------
// Cone (pointed): volume = 1/3·π·r²·h.
// ---------------------------------------------------------------------

macro_rules! cone_volume_test {
    ($name:ident, $r:expr, $h:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_cone(&mut model, $r, 0.0, $h);
            let report = model.mass_properties_for(id).expect("cone mass props");
            let (r, h) = ($r as f64, $h as f64);
            let expected = 1.0 / 3.0 * PI * r * r * h;
            assert!(
                rel_close(report.volume, expected, 0.06),
                "cone r={} h={}: volume {} vs expected {}",
                $r,
                $h,
                report.volume,
                expected
            );
            assert_report_sane!(report);
        }
    };
}

cone_volume_test!(cone_vol_1_1, 1.0, 1.0);
cone_volume_test!(cone_vol_2_5, 2.0, 5.0);
cone_volume_test!(cone_vol_0p5_3, 0.5, 3.0);
cone_volume_test!(cone_vol_3_2, 3.0, 2.0);
cone_volume_test!(cone_vol_5_10, 5.0, 10.0);
cone_volume_test!(cone_vol_1p5_4, 1.5, 4.0);
cone_volume_test!(cone_vol_4_6, 4.0, 6.0);
cone_volume_test!(cone_vol_2p5_2p5, 2.5, 2.5);
cone_volume_test!(cone_vol_6_3, 6.0, 3.0);
cone_volume_test!(cone_vol_1_10, 1.0, 10.0);
cone_volume_test!(cone_vol_3_8, 3.0, 8.0);
cone_volume_test!(cone_vol_2_7, 2.0, 7.0);

// ---------------------------------------------------------------------
// Frustum (truncated cone): volume = 1/3·π·h·(R² + R·r + r²).
// ---------------------------------------------------------------------

macro_rules! frustum_volume_test {
    ($name:ident, $rb:expr, $rt:expr, $h:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_cone(&mut model, $rb, $rt, $h);
            let report = model.mass_properties_for(id).expect("frustum mass props");
            let (rb, rt, h) = ($rb as f64, $rt as f64, $h as f64);
            let expected = 1.0 / 3.0 * PI * h * (rb * rb + rb * rt + rt * rt);
            assert!(
                rel_close(report.volume, expected, 0.06),
                "frustum rb={} rt={} h={}: volume {} vs expected {}",
                $rb,
                $rt,
                $h,
                report.volume,
                expected
            );
            assert_report_sane!(report);
        }
    };
}

frustum_volume_test!(frustum_vol_2_1_4, 2.0, 1.0, 4.0);
frustum_volume_test!(frustum_vol_3_1_5, 3.0, 1.0, 5.0);
frustum_volume_test!(frustum_vol_5_2_3, 5.0, 2.0, 3.0);
frustum_volume_test!(frustum_vol_4_3_6, 4.0, 3.0, 6.0);
frustum_volume_test!(frustum_vol_6_1_2, 6.0, 1.0, 2.0);
frustum_volume_test!(frustum_vol_2p5_0p5_5, 2.5, 0.5, 5.0);
frustum_volume_test!(frustum_vol_8_4_4, 8.0, 4.0, 4.0);
frustum_volume_test!(frustum_vol_3_2_7, 3.0, 2.0, 7.0);

// ---------------------------------------------------------------------
// Property tests: formulas hold across randomised dimensions, and volume
// is strictly monotone in the obvious parameter.
// ---------------------------------------------------------------------

proptest! {
    // 12 cases keeps these tessellation-heavy properties quick (every case
    // builds a primitive and runs the divergence-theorem mass pipeline); the
    // exact formulas are already pinned by the table-driven tests above, so
    // the property pass is a randomised cross-check, not the primary cover.
    // Input ranges stay in the moderate regime the table tests exercise —
    // very large radii/heights tessellate into enough triangles that the mass
    // integration dominates wall-clock without adding coverage.
    #![proptest_config(ProptestConfig::with_cases(12))]

    #[test]
    fn prop_box_volume_matches_formula(
        w in 0.2f64..25.0, h in 0.2f64..25.0, d in 0.2f64..25.0,
    ) {
        let mut model = BRepModel::new();
        let id = make_box(&mut model, w, h, d);
        let r = model.mass_properties_for(id).expect("box mass props");
        prop_assert!(rel_close(r.volume, w * h * d, 0.03),
            "box {w}x{h}x{d}: {} vs {}", r.volume, w * h * d);
        prop_assert!(r.volume > 0.0 && r.mass > 0.0 && r.surface_area > 0.0);
    }

    #[test]
    fn prop_sphere_volume_matches_formula(radius in 0.2f64..12.0) {
        let mut model = BRepModel::new();
        let id = make_sphere(&mut model, radius);
        let r = model.mass_properties_for(id).expect("sphere mass props");
        let expected = 4.0 / 3.0 * PI * radius.powi(3);
        prop_assert!(rel_close(r.volume, expected, 0.05),
            "sphere r={radius}: {} vs {expected}", r.volume);
    }

    #[test]
    fn prop_cylinder_volume_matches_formula(
        radius in 0.3f64..5.0, height in 0.3f64..10.0,
    ) {
        let mut model = BRepModel::new();
        let id = make_cylinder(&mut model, radius, height);
        let r = model.mass_properties_for(id).expect("cylinder mass props");
        let expected = PI * radius * radius * height;
        prop_assert!(rel_close(r.volume, expected, 0.05),
            "cylinder r={radius} h={height}: {} vs {expected}", r.volume);
    }

    #[test]
    fn prop_sphere_volume_monotone_in_radius(
        r1 in 0.2f64..6.0, delta in 0.1f64..6.0,
    ) {
        let r2 = r1 + delta;
        let mut m1 = BRepModel::new();
        let id1 = make_sphere(&mut m1, r1);
        let v1 = m1.mass_properties_for(id1).expect("mp1").volume;
        let mut m2 = BRepModel::new();
        let id2 = make_sphere(&mut m2, r2);
        let v2 = m2.mass_properties_for(id2).expect("mp2").volume;
        prop_assert!(v2 > v1, "sphere volume not monotone: r{r1}->{v1}, r{r2}->{v2}");
    }
}
