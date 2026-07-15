// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Comprehensive boolean DETERMINISM sweep.
//!
//! Every Boolean (∩ / ∪ / ∖) on a primitive pair must be byte-stable run-to-run:
//! `std::HashMap` reseeds its `RandomState` per map per process, so any map whose
//! iteration order leaks into a geometric decision makes the result vary between
//! otherwise-identical calls. Running the SAME Boolean 8 times in one process
//! exercises 8 internal hash seeds; the result volume must be identical every
//! time (a different topology — a dropped face, a flipped fragment, a re-ordered
//! arrangement at a tangency — moves it far more than the 1e-6 relative
//! threshold; sub-1e-6 FP-summation noise is tolerated). A Boolean that errors
//! must do so on every run (a sometimes-Some / sometimes-None split is itself a
//! non-determinism).
//!
//! This is the brutal complement to the curved-boolean poke matrix (which checks
//! correctness, not run-to-run stability) and locks the determinism hardening:
//! the face_arrangement curvature tie-break and the sphere two-cap split that
//! made the degenerate box∩sphere reproducible (#82). It sweeps planar, curved,
//! rotated, and poke configurations across all three operations.

use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::operations::{
    boolean_operation, transform_solid, BooleanOp, BooleanOptions, TransformOptions,
};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

// ---------------------------------------------------------------------------
// Solid builders (each produces a fresh solid in `model`)
// ---------------------------------------------------------------------------

fn box_2(model: &mut BRepModel) -> SolidId {
    match TopologyBuilder::new(model)
        .create_box_3d(2.0, 2.0, 2.0)
        .expect("box")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn box_at(model: &mut BRepModel, c: [f64; 3]) -> SolidId {
    let id = box_2(model);
    transform_solid(
        model,
        id,
        Matrix4::from_translation(&Vector3::new(c[0], c[1], c[2])),
        TransformOptions::default(),
    )
    .expect("translate");
    id
}

fn box_rot_z(model: &mut BRepModel, angle: f64) -> SolidId {
    let id = box_2(model);
    transform_solid(
        model,
        id,
        Matrix4::rotation_z(angle),
        TransformOptions::default(),
    )
    .expect("rotate");
    id
}

fn sphere_at(model: &mut BRepModel, c: [f64; 3], r: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_sphere_3d(Point3::new(c[0], c[1], c[2]), r)
        .expect("sphere")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

fn z_cyl(model: &mut BRepModel, base: [f64; 3], r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(model)
        .create_cylinder_3d(Point3::new(base[0], base[1], base[2]), Vector3::Z, r, h)
        .expect("cylinder")
    {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Determinism harness
// ---------------------------------------------------------------------------

type Build = Box<dyn Fn(&mut BRepModel) -> SolidId>;

/// One Boolean of `op` between fresh `a` and `b`; `None` if it errors. Fresh
/// model per call so neither operand is mutated across the 8 measurements.
fn op_volume(op: BooleanOp, build_a: &Build, build_b: &Build) -> Option<f64> {
    let mut model = BRepModel::new();
    let a = build_a(&mut model);
    let b = build_b(&mut model);
    let r = boolean_operation(&mut model, a, b, op, BooleanOptions::default()).ok()?;
    model.calculate_solid_volume(r)
}

/// Assert `op` on the pair is deterministic across 8 in-process runs.
fn check(op: BooleanOp, label: &str, build_a: &Build, build_b: &Build, fails: &mut Vec<String>) {
    let runs: Vec<Option<f64>> = (0..8).map(|_| op_volume(op, build_a, build_b)).collect();
    let first = runs[0];
    for (i, r) in runs.iter().enumerate() {
        let consistent = match (first, *r) {
            (None, None) => true,
            (Some(a), Some(b)) => (a - b).abs() / a.abs().max(1.0) < 1e-6,
            _ => false, // sometimes-errors, sometimes-succeeds → non-deterministic
        };
        if !consistent {
            fails.push(format!(
                "{label} {op:?}: run 0 = {first:?}, run {i} = {r:?} (all = {runs:?})"
            ));
            break;
        }
    }
}

fn configs() -> Vec<(&'static str, Build, Build)> {
    let r45 = std::f64::consts::FRAC_PI_4;
    let r30 = 30.0_f64 * std::f64::consts::PI / 180.0;
    vec![
        // --- planar ---
        (
            "box ∩/∪/∖ box rot45 (octagonal-prism class)",
            Box::new(box_2),
            Box::new(move |m| box_rot_z(m, r45)),
        ),
        (
            "box vs box rot30",
            Box::new(box_2),
            Box::new(move |m| box_rot_z(m, r30)),
        ),
        (
            "box vs box offset (partial overlap)",
            Box::new(box_2),
            Box::new(|m| box_at(m, [1.0, 0.6, 0.4])),
        ),
        (
            "box vs box corner overlap",
            Box::new(box_2),
            Box::new(|m| box_at(m, [1.5, 1.5, 1.5])),
        ),
        // --- curved: sphere ---
        (
            "box vs sphere centred (curved central)",
            Box::new(box_2),
            Box::new(|m| sphere_at(m, [0.0, 0.0, 0.0], 1.2)),
        ),
        (
            "box vs sphere poking a face",
            Box::new(box_2),
            Box::new(|m| sphere_at(m, [0.7, 0.0, 0.0], 1.0)),
        ),
        (
            "box vs sphere off-axis corner",
            Box::new(box_2),
            Box::new(|m| sphere_at(m, [0.8, 0.8, 0.0], 1.0)),
        ),
        // --- curved: cylinder ---
        (
            "box vs cylinder radial poke (#81 class)",
            Box::new(box_2),
            Box::new(|m| z_cyl(m, [0.0, 0.0, -0.7], 1.2, 1.4)),
        ),
        (
            "box vs cylinder axial poke",
            Box::new(box_2),
            Box::new(|m| z_cyl(m, [0.0, 0.0, 0.0], 0.6, 2.5)),
        ),
        (
            "box vs cylinder contained",
            Box::new(box_2),
            Box::new(|m| z_cyl(m, [0.0, 0.0, -0.5], 0.5, 1.0)),
        ),
        // --- curved: sphere/sphere ---
        (
            "sphere vs sphere overlapping",
            Box::new(|m| sphere_at(m, [0.0, 0.0, 0.0], 1.2)),
            Box::new(|m| sphere_at(m, [1.0, 0.0, 0.0], 1.2)),
        ),
    ]
}

#[test]
fn boolean_intersection_is_deterministic_across_configs() {
    let mut fails = Vec::new();
    for (label, a, b) in configs() {
        check(BooleanOp::Intersection, label, &a, &b, &mut fails);
    }
    assert!(
        fails.is_empty(),
        "non-deterministic ∩:\n  {}",
        fails.join("\n  ")
    );
}

#[test]
fn boolean_union_is_deterministic_across_configs() {
    let mut fails = Vec::new();
    for (label, a, b) in configs() {
        check(BooleanOp::Union, label, &a, &b, &mut fails);
    }
    assert!(
        fails.is_empty(),
        "non-deterministic ∪:\n  {}",
        fails.join("\n  ")
    );
}

#[test]
fn boolean_difference_is_deterministic_across_configs() {
    let mut fails = Vec::new();
    for (label, a, b) in configs() {
        check(BooleanOp::Difference, label, &a, &b, &mut fails);
    }
    assert!(
        fails.is_empty(),
        "non-deterministic ∖:\n  {}",
        fails.join("\n  ")
    );
}
