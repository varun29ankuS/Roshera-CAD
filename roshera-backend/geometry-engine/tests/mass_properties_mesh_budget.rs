// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! IMP-2: the mesh that mass properties integrate over scales with the part,
//! and its error stays bounded by each feature, not by the part.
//!
//! `fine()` carries an ABSOLUTE 0.01 mm maximum edge length, so on a 20 mm
//! gear from the robot-arm assembly every curved face hit the 200-segment
//! cap: 1.7 million triangles, 1006 s (debug). The edge length is now
//! floored at 0.5 % of the part. That floor exposed a tessellation defect
//! the tiny edge length had hidden: a curved face's interior grid ignored
//! the angle limit its boundary edges honour, so a small bore in a large
//! part was chorded coarsely between finely sampled boundary circles.

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

fn solid(id: GeometryId) -> SolidId {
    match id {
        GeometryId::Solid(s) => s,
        other => panic!("expected a solid, got {other:?}"),
    }
}

/// A 20 mm cube with a 4 mm through-bore along z: trimmed faces, so its
/// mass properties take the mesh path, with straight edges well short of
/// the part's diagonal and an exact answer to check against.
fn bored_block() -> (BRepModel, SolidId) {
    let mut model = BRepModel::new();
    let block = solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(20.0, 20.0, 20.0)
            .expect("block"),
    );
    let bore = solid(
        TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(0.0, 0.0, -15.0),
                Vector3::new(0.0, 0.0, 1.0),
                4.0,
                30.0,
            )
            .expect("bore"),
    );
    let result = boolean_operation(
        &mut model,
        block,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bored block");
    (model, result)
}

fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    mesh.triangles
        .iter()
        .map(|t| {
            let a = mesh.vertices[t[0] as usize].position;
            let b = mesh.vertices[t[1] as usize].position;
            let c = mesh.vertices[t[2] as usize].position;
            (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
                + a.z * (b.x * c.y - b.y * c.x))
                / 6.0
        })
        .sum()
}

#[test]
fn the_mass_properties_mesh_edge_length_is_floored_to_the_part_size() {
    let (model, id) = bored_block();
    let part = model.solids.get(id).expect("part");
    let params = TessellationParams::mass_properties(part, &model);
    let fine = TessellationParams::fine();
    assert!(
        params.max_edge_length > fine.max_edge_length,
        "a 20 mm part floors the 0.01 mm edge length: got {}",
        params.max_edge_length
    );
    // Only the edge length moves: the accuracy controls stay `fine()`'s.
    assert_eq!(params.chord_tolerance, fine.chord_tolerance);
    assert_eq!(params.max_angle_deviation, fine.max_angle_deviation);
    assert_eq!(params.max_segments, fine.max_segments);
}

#[test]
fn volume_is_integrated_on_the_size_floored_mesh() {
    let (mut model, id) = bored_block();
    let expected = {
        let part = model.solids.get(id).expect("part");
        let params = TessellationParams::mass_properties(part, &model);
        mesh_volume(&tessellate_solid(part, &model, &params))
    };
    let volume = model.calculate_solid_volume(id).expect("volume");
    assert!(
        (volume - expected).abs() <= 1e-9 * expected.abs(),
        "mass properties must integrate the size-floored mesh: \
         volume {volume} vs that mesh's {expected}"
    );
}

/// The error a mesh puts on a feature must be bounded by the feature, not
/// by the part. An 80 × 40 × 40 plate with one Ø2 through-bore: floored to
/// the part's size, the bore wall's interior grid took about 13 segments
/// around while its boundary circles took 315, and the volume came out
/// 2.76 mm³ high, 2.2 % of the bore's own 125.7 mm³. `fine()` gives
/// 0.018 mm³. The bound here is 0.1 % of the feature.
#[test]
fn a_small_bore_in_a_large_part_keeps_its_own_accuracy() {
    let mut model = BRepModel::new();
    let plate = solid(
        TopologyBuilder::new(&mut model)
            .create_box_3d(80.0, 40.0, 40.0)
            .expect("plate"),
    );
    let bore = solid(
        TopologyBuilder::new(&mut model)
            .create_cylinder_3d(
                Point3::new(-30.0, 0.0, -30.0),
                Vector3::new(0.0, 0.0, 1.0),
                1.0,
                60.0,
            )
            .expect("bore"),
    );
    let id = boolean_operation(
        &mut model,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("bored plate");
    let bore_volume = std::f64::consts::PI * 40.0;
    let exact = 80.0 * 40.0 * 40.0 - bore_volume;
    let volume = model.calculate_solid_volume(id).expect("volume");
    let error = (volume - exact).abs();
    assert!(
        error <= 1e-3 * bore_volume,
        "volume {volume} vs exact {exact}: the {error} mm³ error is {:.3} % of the bore it comes from",
        100.0 * error / bore_volume
    );
}
