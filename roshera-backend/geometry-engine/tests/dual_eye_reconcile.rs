//! Integration tests for the dual-eye reconcile dimension of `certify_solid`.
//!
//! Task 5: `certify_solid` must compute the real `eyes_consistent` value by
//! running `recognize_features` → `check_eyes_consistent` rather than returning
//! the `NotApplicable` placeholder.
//!
//! ## Test strategy (non-vacuous)
//! A bare box has no recognizable features → `recognize_features` returns `[]`
//! → `check_eyes_consistent` → `NotApplicable`. The Task-4 placeholder already
//! returns `NotApplicable`, so a bare-box test would pass vacuously (it proves
//! nothing about the real computation).
//!
//! A standalone cylinder IS recognized: its cylindrical lateral face is
//! adjacent to two planar caps → `recognize_features` emits a `ThroughHole`
//! or `CylindricalBoss` entry → `check_eyes_consistent` calls `all_live ==
//! true` → `Consistent`. The placeholder returns `NotApplicable` ≠ `Consistent`
//! → the test is genuinely RED before the producer edit and GREEN after.

#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::provenance::EyesConsistency;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Build a standalone cylinder (radius 10, height 20) inside `model`.
/// The cylindrical lateral face is adjacent to the two planar cap faces,
/// so `recognize_features` will always emit at least one cylindrical feature.
fn make_cylinder(model: &mut BRepModel) -> u32 {
    let mut b = TopologyBuilder::new(model);
    match b
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 10.0, 20.0)
        .expect("create_cylinder_3d")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid, got {other:?}"),
    }
}

/// Build a standalone box (10 × 10 × 10) inside `model`.
fn make_box(model: &mut BRepModel) -> u32 {
    let mut b = TopologyBuilder::new(model);
    match b.create_box_3d(10.0, 10.0, 10.0).expect("create_box_3d") {
        GeometryId::Solid(id) => id,
        other => panic!("expected Solid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Non-vacuous test: cylinder has a recognizable cylindrical feature.
// BEFORE the Task-5 producer edit this returns `NotApplicable` (placeholder)
// and the assertion `== Consistent` FAILS → genuinely RED.
// AFTER the edit the producer calls `recognize_features` + `check_eyes_consistent`
// and returns `Consistent` → GREEN.
// ---------------------------------------------------------------------------
#[test]
fn certify_sets_eyes_consistent_on_a_sound_part() {
    let mut model = BRepModel::new();
    let solid_id = make_cylinder(&mut model);
    let cert = model.certify_solid(solid_id);

    // The cylinder is sound (closed, manifold, oriented, etc.).
    assert!(cert.is_sound(), "cylinder must certify sound: {cert:?}");

    // `recognize_features` emits at least one cylindrical feature (through-hole
    // or cylindrical-boss heuristic), so `check_eyes_consistent` returns
    // `Consistent` (all feature face-ids are live). The placeholder `NotApplicable`
    // would fail this assertion.
    assert_eq!(
        cert.eyes_consistent,
        EyesConsistency::Consistent,
        "cylinder has a recognized feature → eyes_consistent must be Consistent, got {:?}",
        cert.eyes_consistent,
    );
}

// ---------------------------------------------------------------------------
// Companion / regression guard: a bare box has no recognizable features →
// `NotApplicable` (the placeholder value happens to match, but the test now
// lives alongside the real computation, not in place of it).
// ---------------------------------------------------------------------------
#[test]
fn bare_box_has_no_features_so_not_applicable() {
    let mut model = BRepModel::new();
    let solid_id = make_box(&mut model);
    let cert = model.certify_solid(solid_id);

    assert!(cert.is_sound(), "bare box must certify sound: {cert:?}");

    // A box has only planar faces — `recognize_features` returns `[]` →
    // `check_eyes_consistent` → `NotApplicable`.
    assert_eq!(
        cert.eyes_consistent,
        EyesConsistency::NotApplicable,
        "bare box: no recognized features → NotApplicable, got {:?}",
        cert.eyes_consistent,
    );
}
