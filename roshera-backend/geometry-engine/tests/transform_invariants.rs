// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Mass-property invariants under affine transforms.
//!
//! `transform_solid` mutates a solid's B-Rep in place, so re-querying
//! `mass_properties_for` after a transform must obey the elementary rules:
//! a rigid motion (rotation + translation) preserves volume and surface
//! area; a pure translation shifts the centre of mass by exactly the
//! translation vector; a scale by (sx, sy, sz) multiplies volume by
//! sx·sy·sz (and a uniform scale s multiplies area by s²).
//!
//! Most cases use a box: its faces stay planar under any affine map, so
//! the tessellated volume/area integrate near-exactly and we can assert a
//! tight 3 % tolerance. A few curved cases (sphere, cylinder) use the
//! 5 % tessellation ceiling. COM shifts are positional and mesh-stable, so
//! a small absolute tolerance applies.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

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

fn rel_close(actual: f64, expected: f64, rel_tol: f64) -> bool {
    if expected == 0.0 {
        return actual.abs() <= rel_tol;
    }
    ((actual - expected) / expected).abs() <= rel_tol
}

fn apply(model: &mut BRepModel, id: SolidId, m: Matrix4) {
    transform_solid(model, id, m, TransformOptions::default()).expect("transform succeeds");
}

fn rigid(angle: f64, tx: f64, ty: f64, tz: f64) -> Matrix4 {
    let r = Matrix4::from_axis_angle(&Vector3::new(1.0, 2.0, 3.0).normalize_or_zero(), angle)
        .expect("axis-angle");
    Matrix4::from_translation(&Vector3::new(tx, ty, tz)) * r
}

// =====================================================================
// Rigid motion preserves volume and surface area (box: near-exact).
// =====================================================================

macro_rules! rigid_preserves_volume_box {
    ($name:ident, $w:expr, $h:expr, $d:expr, $ang:expr, $tx:expr, $ty:expr, $tz:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let v0 = model.mass_properties_for(id).expect("mp before").volume;
            apply(&mut model, id, rigid($ang, $tx, $ty, $tz));
            let v1 = model.mass_properties_for(id).expect("mp after").volume;
            assert!(
                rel_close(v1, v0, 0.03),
                "rigid motion changed volume: {v0} -> {v1}"
            );
        }
    };
}

rigid_preserves_volume_box!(rigid_vol_unit, 1.0, 1.0, 1.0, 0.7, 0.0, 0.0, 0.0);
rigid_preserves_volume_box!(rigid_vol_2_3_4, 2.0, 3.0, 4.0, 1.2, 5.0, -2.0, 1.0);
rigid_preserves_volume_box!(rigid_vol_flat, 10.0, 1.0, 6.0, 2.0, -3.0, 4.0, 2.0);
rigid_preserves_volume_box!(
    rigid_vol_pi,
    3.0,
    3.0,
    3.0,
    std::f64::consts::PI,
    1.0,
    1.0,
    1.0
);
rigid_preserves_volume_box!(rigid_vol_neg, 5.0, 2.0, 7.0, -1.4, -10.0, 0.0, 8.0);
rigid_preserves_volume_box!(rigid_vol_big, 8.0, 8.0, 2.0, 0.3, 100.0, -50.0, 25.0);
rigid_preserves_volume_box!(rigid_vol_small_ang, 4.0, 6.0, 1.0, 0.01, 1.0, 2.0, 3.0);
rigid_preserves_volume_box!(rigid_vol_tall, 1.0, 12.0, 1.0, 2.5, 0.0, -5.0, 0.0);
rigid_preserves_volume_box!(rigid_vol_frac, 1.5, 2.5, 3.5, 1.1, 2.2, 3.3, 4.4);
rigid_preserves_volume_box!(rigid_vol_pure_rot, 6.0, 4.0, 5.0, 2.2, 0.0, 0.0, 0.0);

macro_rules! rigid_preserves_area_box {
    ($name:ident, $w:expr, $h:expr, $d:expr, $ang:expr, $tx:expr, $ty:expr, $tz:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let a0 = model
                .mass_properties_for(id)
                .expect("mp before")
                .surface_area;
            apply(&mut model, id, rigid($ang, $tx, $ty, $tz));
            let a1 = model
                .mass_properties_for(id)
                .expect("mp after")
                .surface_area;
            assert!(
                rel_close(a1, a0, 0.03),
                "rigid motion changed surface area: {a0} -> {a1}"
            );
        }
    };
}

rigid_preserves_area_box!(rigid_area_unit, 1.0, 1.0, 1.0, 0.7, 1.0, 2.0, 3.0);
rigid_preserves_area_box!(rigid_area_2_3_4, 2.0, 3.0, 4.0, 1.2, -5.0, 2.0, -1.0);
rigid_preserves_area_box!(rigid_area_flat, 10.0, 1.0, 6.0, 2.0, 3.0, -4.0, 2.0);
rigid_preserves_area_box!(rigid_area_cube, 3.0, 3.0, 3.0, 3.0, 0.0, 0.0, 0.0);
rigid_preserves_area_box!(rigid_area_big, 8.0, 8.0, 2.0, 0.6, 20.0, 20.0, -20.0);
rigid_preserves_area_box!(rigid_area_tall, 1.0, 12.0, 1.0, 2.5, -1.0, 5.0, 1.0);
rigid_preserves_area_box!(rigid_area_frac, 1.5, 2.5, 3.5, 1.1, 2.0, 3.0, 4.0);
rigid_preserves_area_box!(rigid_area_pure_rot, 6.0, 4.0, 5.0, 2.2, 0.0, 0.0, 0.0);

// =====================================================================
// Rotation preserves volume on curved solids (5% tessellation ceiling).
// =====================================================================

// Regression guard: `transform_solid`'s post-transform validation used to
// reject a single-face sphere on the polyhedral Euler-characteristic check
// (V(0)-E(0)+F(1)=1 vs expected 2). That formula doesn't apply to a seamless
// periodic face with no bounding edges; the validator now skips the check when
// E==0 (see validation.rs::validate_euler_characteristic_for_solid).
#[test]
fn rotation_preserves_sphere_volume() {
    for &ang in &[0.5, 1.5, 3.0] {
        let mut model = BRepModel::new();
        let id = make_sphere(&mut model, 3.0);
        let v0 = model.mass_properties_for(id).expect("before").volume;
        apply(
            &mut model,
            id,
            Matrix4::from_axis_angle(&Vector3::new(1.0, 1.0, 0.0).normalize_or_zero(), ang)
                .expect("rot"),
        );
        let v1 = model.mass_properties_for(id).expect("after").volume;
        assert!(
            rel_close(v1, v0, 0.05),
            "sphere rot vol {v0} -> {v1} @ {ang}"
        );
    }
}

// Regression guard for the rotated-cylinder volume bug: a rigid (det = 1)
// transform must preserve volume, but rotating a cylinder off the Z axis used
// to collapse the reported volume to exactly 1/3 (the cone value). Root cause:
// the transform desyncs the lateral seam from the cap circles' t=0, so
// curved-CDT fails with CdtFailed(PointOnFixedEdge) and the old empty-mesh
// fallback dropped the entire lateral wall. Fixed by falling back to an
// un-trimmed analytic grid (tessellation/surface.rs).
#[test]
fn rigid_preserves_cylinder_volume() {
    for &(ang, tx) in &[(0.4, 5.0), (1.7, -3.0), (2.9, 10.0)] {
        let mut model = BRepModel::new();
        let id = make_cylinder(&mut model, 2.0, 6.0);
        let v0 = model.mass_properties_for(id).expect("before").volume;
        apply(&mut model, id, rigid(ang, tx, 0.0, 0.0));
        let v1 = model.mass_properties_for(id).expect("after").volume;
        assert!(rel_close(v1, v0, 0.05), "cyl rigid vol {v0} -> {v1}");
    }
}

// =====================================================================
// Pure translation shifts the centre of mass by exactly the vector.
// =====================================================================

macro_rules! translation_shifts_com {
    ($name:ident, $w:expr, $h:expr, $d:expr, $tx:expr, $ty:expr, $tz:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let c0 = model
                .mass_properties_for(id)
                .expect("before")
                .center_of_mass;
            apply(
                &mut model,
                id,
                Matrix4::from_translation(&Vector3::new($tx, $ty, $tz)),
            );
            let c1 = model.mass_properties_for(id).expect("after").center_of_mass;
            let shift = [$tx as f64, $ty as f64, $tz as f64];
            for k in 0..3 {
                let got = c1[k] - c0[k];
                assert!(
                    (got - shift[k]).abs() <= 1e-3 * (1.0 + shift[k].abs()),
                    "COM axis {k} shifted {got}, expected {}",
                    shift[k]
                );
            }
        }
    };
}

translation_shifts_com!(com_shift_x, 2.0, 2.0, 2.0, 5.0, 0.0, 0.0);
translation_shifts_com!(com_shift_y, 2.0, 2.0, 2.0, 0.0, 7.0, 0.0);
translation_shifts_com!(com_shift_z, 2.0, 2.0, 2.0, 0.0, 0.0, -4.0);
translation_shifts_com!(com_shift_xyz, 3.0, 4.0, 5.0, 1.0, 2.0, 3.0);
translation_shifts_com!(com_shift_neg, 1.0, 6.0, 2.0, -8.0, 3.0, -2.0);
translation_shifts_com!(com_shift_big, 4.0, 4.0, 4.0, 100.0, -50.0, 25.0);
translation_shifts_com!(com_shift_frac, 2.5, 1.5, 3.5, 0.5, -1.5, 2.5);
translation_shifts_com!(com_shift_flat, 10.0, 1.0, 8.0, 2.0, 2.0, 2.0);

// =====================================================================
// Scale multiplies volume by the product of factors.
// =====================================================================

macro_rules! uniform_scale_volume {
    ($name:ident, $w:expr, $h:expr, $d:expr, $s:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let v0 = model.mass_properties_for(id).expect("before").volume;
            let a0 = model.mass_properties_for(id).expect("before").surface_area;
            let s = $s as f64;
            apply(&mut model, id, Matrix4::scale(s, s, s));
            let mp = model.mass_properties_for(id).expect("after");
            assert!(
                rel_close(mp.volume, v0 * s * s * s, 0.03),
                "uniform scale {s}: vol {} vs {}",
                mp.volume,
                v0 * s * s * s
            );
            assert!(
                rel_close(mp.surface_area, a0 * s * s, 0.03),
                "uniform scale {s}: area {} vs {}",
                mp.surface_area,
                a0 * s * s
            );
        }
    };
}

uniform_scale_volume!(uscale_2, 2.0, 2.0, 2.0, 2.0);
uniform_scale_volume!(uscale_3, 1.0, 2.0, 3.0, 3.0);
uniform_scale_volume!(uscale_half, 4.0, 4.0, 4.0, 0.5);
uniform_scale_volume!(uscale_1p5, 2.0, 3.0, 1.0, 1.5);
uniform_scale_volume!(uscale_5, 1.0, 1.0, 1.0, 5.0);
uniform_scale_volume!(uscale_0p25, 8.0, 8.0, 8.0, 0.25);
uniform_scale_volume!(uscale_2p5, 3.0, 2.0, 4.0, 2.5);
uniform_scale_volume!(uscale_10, 1.0, 1.0, 1.0, 10.0);

macro_rules! nonuniform_scale_volume {
    ($name:ident, $w:expr, $h:expr, $d:expr, $sx:expr, $sy:expr, $sz:expr) => {
        #[test]
        fn $name() {
            let mut model = BRepModel::new();
            let id = make_box(&mut model, $w, $h, $d);
            let v0 = model.mass_properties_for(id).expect("before").volume;
            let (sx, sy, sz) = ($sx as f64, $sy as f64, $sz as f64);
            apply(&mut model, id, Matrix4::scale(sx, sy, sz));
            let v1 = model.mass_properties_for(id).expect("after").volume;
            assert!(
                rel_close(v1, v0 * sx * sy * sz, 0.03),
                "scale ({sx},{sy},{sz}): vol {v1} vs {}",
                v0 * sx * sy * sz
            );
        }
    };
}

nonuniform_scale_volume!(nuscale_a, 2.0, 2.0, 2.0, 2.0, 3.0, 4.0);
nonuniform_scale_volume!(nuscale_b, 1.0, 1.0, 1.0, 5.0, 0.5, 2.0);
nonuniform_scale_volume!(nuscale_c, 3.0, 2.0, 1.0, 1.5, 1.5, 1.5);
nonuniform_scale_volume!(nuscale_d, 4.0, 4.0, 4.0, 0.25, 4.0, 1.0);
nonuniform_scale_volume!(nuscale_e, 2.0, 5.0, 3.0, 3.0, 0.5, 2.5);
nonuniform_scale_volume!(nuscale_f, 1.0, 2.0, 3.0, 10.0, 1.0, 0.1);

// =====================================================================
// Property tests over randomised transforms.
// =====================================================================

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(16))]

    #[test]
    fn prop_rigid_preserves_box_volume(
        w in 0.5f64..12.0, h in 0.5f64..12.0, d in 0.5f64..12.0,
        angle in -3.0f64..3.0,
        tx in -20.0f64..20.0, ty in -20.0f64..20.0, tz in -20.0f64..20.0,
    ) {
        let mut model = BRepModel::new();
        let id = make_box(&mut model, w, h, d);
        let v0 = model.mass_properties_for(id).expect("before").volume;
        apply(&mut model, id, rigid(angle, tx, ty, tz));
        let v1 = model.mass_properties_for(id).expect("after").volume;
        prop_assert!(rel_close(v1, v0, 0.03), "{v0} -> {v1}");
    }

    #[test]
    fn prop_translation_shifts_box_com(
        w in 0.5f64..10.0, h in 0.5f64..10.0, d in 0.5f64..10.0,
        tx in -30.0f64..30.0, ty in -30.0f64..30.0, tz in -30.0f64..30.0,
    ) {
        let mut model = BRepModel::new();
        let id = make_box(&mut model, w, h, d);
        let c0 = model.mass_properties_for(id).expect("before").center_of_mass;
        apply(&mut model, id, Matrix4::from_translation(&Vector3::new(tx, ty, tz)));
        let c1 = model.mass_properties_for(id).expect("after").center_of_mass;
        let shift = [tx, ty, tz];
        for k in 0..3 {
            let got = c1[k] - c0[k];
            prop_assert!((got - shift[k]).abs() <= 1e-2 * (1.0 + shift[k].abs()),
                "axis {k}: got {got} expected {}", shift[k]);
        }
    }

    #[test]
    fn prop_uniform_scale_cubes_box_volume(
        w in 0.5f64..8.0, h in 0.5f64..8.0, d in 0.5f64..8.0,
        s in 0.3f64..4.0,
    ) {
        let mut model = BRepModel::new();
        let id = make_box(&mut model, w, h, d);
        let v0 = model.mass_properties_for(id).expect("before").volume;
        apply(&mut model, id, Matrix4::scale(s, s, s));
        let v1 = model.mass_properties_for(id).expect("after").volume;
        prop_assert!(rel_close(v1, v0 * s * s * s, 0.03), "scale {s}: {v1} vs {}", v0 * s * s * s);
    }
}
