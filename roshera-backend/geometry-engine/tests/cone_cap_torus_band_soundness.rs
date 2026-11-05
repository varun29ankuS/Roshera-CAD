// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! The cone and torus meshers changed 2026-11-05 (apex-cap fan, two-circle
//! torus band) for faces the robot-arm STEP import brought in. The kernel's
//! own cone, frustum and torus primitives must stay sound where they are built
//! AND after rigid placements: a move is an isometry and cannot honestly change
//! soundness. (Booleans that MAKE those face shapes in-kernel are covered by
//! `boolean_cone_torus_plane_cuts.rs`.)

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn sid(g: GeometryId) -> SolidId {
    match g {
        GeometryId::Solid(id) => id,
        o => panic!("expected a solid, got {o:?}"),
    }
}

/// Rotation by `angle` about the unit axis, then translation.
fn placement(axis: [f64; 3], angle: f64, t: [f64; 3]) -> Matrix4 {
    let [x, y, z] = axis;
    let (s, c) = angle.sin_cos();
    let k = 1.0 - c;
    Matrix4::from_cols(
        Vector3::new(c + x * x * k, y * x * k + z * s, z * x * k - y * s),
        Vector3::new(x * y * k - z * s, c + y * y * k, z * y * k + x * s),
        Vector3::new(x * z * k + y * s, y * z * k - x * s, c + z * z * k),
        Vector3::new(t[0], t[1], t[2]),
    )
}

fn assert_sound(model: &mut BRepModel, s: SolidId, what: &str) {
    let cert = model.certify_solid(s);
    assert!(
        cert.is_sound(),
        "{what}: must certify sound (brep_valid={} watertight={} oriented={} \
         self_x_free={}): {:?}",
        cert.brep_valid,
        cert.watertight,
        cert.oriented,
        cert.self_intersection_free,
        cert.errors
    );
}

/// Sound as built, then sound after each of three placements (applied in turn).
fn assert_sound_built_and_placed(model: &mut BRepModel, s: SolidId, what: &str) {
    assert_sound(model, s, &format!("{what} as built"));
    let moves = [
        (
            "rotate-x 90",
            placement([1.0, 0.0, 0.0], std::f64::consts::FRAC_PI_2, [0.0; 3]),
        ),
        (
            "oblique 1.1 rad + move",
            placement([0.3, 0.4, 0.866_025_403_784_438_6], 1.1, [40.0, -15.0, 7.0]),
        ),
        (
            "rotate-z 0.37 rad",
            placement([0.0, 0.0, 1.0], 0.37, [0.0; 3]),
        ),
    ];
    for (name, m) in moves {
        transform_solid(model, s, m, TransformOptions::default()).expect("transform");
        assert_sound(model, s, &format!("{what} after {name}"));
    }
}

#[test]
fn primitive_cone_frustum_and_torus_stay_sound_placed() {
    let mut m = BRepModel::new();
    let cone = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, 8.0, 0.0, 10.0)
        .expect("cone"));
    assert_sound_built_and_placed(&mut m, cone, "primitive cone");

    let frustum = sid(TopologyBuilder::new(&mut m)
        .create_cone_3d(Point3::ORIGIN, Vector3::Z, 8.0, 3.0, 10.0)
        .expect("frustum"));
    assert_sound_built_and_placed(&mut m, frustum, "primitive frustum");

    let torus = sid(TopologyBuilder::new(&mut m)
        .create_torus_3d(Point3::ORIGIN, Vector3::Z, 10.0, 3.0)
        .expect("torus"));
    assert_sound_built_and_placed(&mut m, torus, "primitive torus");
}
