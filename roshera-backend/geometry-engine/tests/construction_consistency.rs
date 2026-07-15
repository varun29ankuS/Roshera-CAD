// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! FIX 1 / FIX 2 gate — cross-entity consistency between a solid and its
//! linked construction geometry (the source sketch).
//!
//! The live defect: after a transform the solid's vertices moved but its
//! construction sketch stayed behind (~17u apart), and `ground_truth` STILL
//! reported `sound=true`. Two failures, both gated here:
//!
//!   * FIX 2 — the certificate is solid-LOCAL only; it must also certify
//!     that the linked construction geometry is co-located with the solid.
//!     Tri-state (Consistent / Inconsistent / NotApplicable) so it is honest.
//!   * FIX 1 — `transform_solid` must carry the construction geometry with
//!     the solid so they never diverge in the first place.
//!
//! The whole point is that case (a) MUST be able to fail: a consistency
//! check that can never report Inconsistent is worthless.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::transform::{transform_solid, TransformOptions};
use geometry_engine::primitives::provenance::{ConstructionConsistency, ConstructionGeometry};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Build a unit-ish box at the origin and link a co-located construction
/// sketch on its bottom face (z = -half). Returns the model + solid id.
fn box_with_colocated_sketch() -> (BRepModel, u32) {
    let mut m = BRepModel::new();
    let solid_id = match TopologyBuilder::new(&mut m)
        .create_box_3d(20.0, 20.0, 20.0)
        .expect("create_box_3d")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    // The box is centred at the origin: x,y,z ∈ [-10, 10]. A sketch on the
    // bottom face is co-located: origin + four corners all lie within the
    // solid bbox.
    let construction = ConstructionGeometry::new(
        Point3::new(0.0, 0.0, -10.0),
        vec![
            Point3::new(-8.0, -8.0, -10.0),
            Point3::new(8.0, -8.0, -10.0),
            Point3::new(8.0, 8.0, -10.0),
            Point3::new(-8.0, 8.0, -10.0),
        ],
    );
    m.set_solid_construction(solid_id, construction);
    (m, solid_id)
}

/// CASE (a) — the MUST-FAIL case. A solid whose construction sketch has been
/// orphaned (displaced ~17u away) certifies `Inconsistent` and `sound=false`.
/// This is what the OLD transform path produced: solid moved, sketch behind.
#[test]
fn orphaned_sketch_is_inconsistent_and_unsound() {
    let (mut m, solid_id) = box_with_colocated_sketch();

    // Sanity: as built, the pair is consistent and sound.
    let before = m.certify_solid(solid_id);
    assert_eq!(
        before.construction_consistent,
        ConstructionConsistency::Consistent,
        "as-built sketch must be co-located"
    );
    assert!(before.is_sound(), "as-built solid must be sound");

    // Orphan the sketch: displace its geometry ~17u in +x WITHOUT moving the
    // solid — exactly the divergence the old transform left behind.
    let orphan = ConstructionGeometry::new(
        Point3::new(17.0, 0.0, -10.0),
        vec![
            Point3::new(9.0, -8.0, -10.0),
            Point3::new(25.0, -8.0, -10.0),
            Point3::new(25.0, 8.0, -10.0),
            Point3::new(9.0, 8.0, -10.0),
        ],
    );
    m.set_solid_construction(solid_id, orphan);

    let cert = m.certify_solid(solid_id);
    assert_eq!(
        cert.construction_consistent,
        ConstructionConsistency::Inconsistent,
        "an orphaned sketch ~17u from the solid MUST be Inconsistent"
    );
    assert!(
        !cert.is_sound(),
        "Inconsistent construction geometry MUST fold sound→false; \
         got cert={cert:?}"
    );
    // The local checks are unaffected — the solid itself is still a fine box.
    assert!(cert.brep_valid && cert.watertight && cert.manifold);
}

/// CASE (b) — with FIX 1, transforming the solid carries the sketch with it,
/// so the pair stays Consistent and the solid stays sound. The OLD path
/// (which never touched construction geometry) would have orphaned it.
#[test]
fn transform_carries_sketch_and_stays_consistent() {
    let (mut m, solid_id) = box_with_colocated_sketch();

    // Rotate 180° about Z (the live repro), then translate well away.
    let rot =
        Matrix4::rotation_axis(Point3::ORIGIN, Vector3::Z, std::f64::consts::PI).expect("rotation");
    let trans = Matrix4::translation(50.0, -30.0, 12.0);
    let transform = trans * rot;

    transform_solid(&mut m, solid_id, transform, TransformOptions::default())
        .expect("transform_solid");

    let cert = m.certify_solid(solid_id);
    assert_eq!(
        cert.construction_consistent,
        ConstructionConsistency::Consistent,
        "FIX 1: the sketch must move WITH the solid, staying co-located; \
         got cert={cert:?}"
    );
    assert!(
        cert.is_sound(),
        "transformed solid with its sketch carried along must stay sound; \
         got cert={cert:?}"
    );
}

/// Direct check that the stored construction geometry actually moved under
/// the transform (FIX 1 mechanism), not merely that the verdict is benign.
#[test]
fn transform_actually_moves_construction_points() {
    let (mut m, solid_id) = box_with_colocated_sketch();
    let before = m
        .solid_construction(solid_id)
        .expect("construction linked")
        .clone();

    let transform = Matrix4::translation(100.0, 0.0, 0.0);
    transform_solid(&mut m, solid_id, transform, TransformOptions::default())
        .expect("transform_solid");

    let after = m
        .solid_construction(solid_id)
        .expect("construction still linked after transform");
    assert!(
        (after.plane_origin.x - before.plane_origin.x - 100.0).abs() < 1e-9,
        "plane origin must translate by +100 in x"
    );
    for (b, a) in before
        .profile_points
        .iter()
        .zip(after.profile_points.iter())
    {
        assert!(
            (a.x - b.x - 100.0).abs() < 1e-9,
            "every profile point must translate by +100 in x"
        );
    }
}

/// CASE (c) — a solid with NO linked sketch (a bare primitive) reports
/// `NotApplicable` and stays sound. Proves no regression: revolve / loft /
/// primitive solids that never had a sketch are never dragged unsound by the
/// new invariant.
#[test]
fn primitive_without_sketch_is_not_applicable_and_sound() {
    let mut m = BRepModel::new();
    let solid_id = match TopologyBuilder::new(&mut m)
        .create_box_3d(10.0, 10.0, 10.0)
        .expect("create_box_3d")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
    let cert = m.certify_solid(solid_id);
    assert_eq!(
        cert.construction_consistent,
        ConstructionConsistency::NotApplicable,
        "a primitive with no linked sketch must be NotApplicable"
    );
    assert!(
        cert.is_sound(),
        "NotApplicable must NOT block soundness — sketch-less solids stay sound"
    );
}

/// `NotApplicable.is_sound()` must be true and `Inconsistent.is_sound()` must
/// be false — the tri-state contract, pinned directly.
#[test]
fn tri_state_soundness_contract() {
    assert!(ConstructionConsistency::Consistent.is_sound());
    assert!(ConstructionConsistency::NotApplicable.is_sound());
    assert!(!ConstructionConsistency::Inconsistent.is_sound());
}
