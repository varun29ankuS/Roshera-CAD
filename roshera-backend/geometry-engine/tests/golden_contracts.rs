// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! PILLAR 2 — golden contract regression. For a set of CANONICAL builds (the
//! cases that must always work), pin the full invariant contract
//! (`harness::integration::full_contract`: B-Rep clean + mesh manifold + Euler +
//! watertight-by-volume + non-degenerate tessellation + determinism) AND the
//! kernel's ground-truth provenance + soundness. A fix that silently breaks any
//! working case fails here loudly — the "renders fine ≠ is valid" guard the
//! kernel needs while the parts are still toys.
//!
//! This deliberately asserts the SIGNATURE (invariants + provenance kind + Euler
//! + pinned volume within tolerance), not a byte-exact mesh, so it is robust to
//! tessellation jitter while still catching real regressions.

use geometry_engine::harness::integration::full_contract;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::nurbs_loft::{nurbs_loft, NurbsLoftOptions};
use geometry_engine::primitives::provenance::OperationKind;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

fn cyl(m: &mut BRepModel, base: Point3, r: f64, h: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, Vector3::Z, r, h)
        .unwrap()
    {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}
fn boxs(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).unwrap() {
        GeometryId::Solid(s) => s,
        o => panic!("{o:?}"),
    }
}

/// Assert the full invariant contract + ground-truth signature for a canonical
/// build. `expect_kind` is the provenance the kernel must report; `volume` the
/// pinned analytic volume (within `vol_tol` relative).
fn assert_golden(
    m: &mut BRepModel,
    solid: SolidId,
    label: &str,
    expect_kind: &OperationKind,
    volume: f64,
    vol_tol: f64,
) {
    // 1. Full invariant contract holds (every oracle layer).
    let c = full_contract(m, solid, 0.05, 0.03);
    assert!(
        c.failures().is_empty(),
        "[{label}] invariant contract broken: {:?}",
        c.failures()
    );

    // 2. Kernel ground truth: provenance matches + the solid certifies SOUND.
    let gt = m
        .ground_truth(solid)
        .unwrap_or_else(|| panic!("[{label}] no ground truth"));
    let prov = gt
        .provenance
        .as_ref()
        .unwrap_or_else(|| panic!("[{label}] no provenance: {}", gt.summary()));
    assert_eq!(
        &prov.created_by,
        expect_kind,
        "[{label}] provenance mismatch: {}",
        gt.summary()
    );
    assert!(
        gt.certificate.is_sound(),
        "[{label}] kernel certificate not sound: {}",
        gt.summary()
    );

    // 3. Pinned analytic volume (catches a silent geometry regression that still
    //    passes topology — a leak/flip/over-inclusion shifts the volume).
    let vol = m.calculate_solid_volume(solid).unwrap_or(0.0);
    assert!(
        (vol - volume).abs() <= vol_tol * volume,
        "[{label}] volume regressed: {vol:.1} vs pinned {volume:.1}"
    );
}

#[test]
fn golden_box() {
    let mut m = BRepModel::new();
    let s = boxs(&mut m, 40.0, 30.0, 20.0);
    assert_golden(
        &mut m,
        s,
        "box 40x30x20",
        &OperationKind::Primitive(geometry_engine::primitives::persistent_id::PrimitiveKind::Box),
        24_000.0,
        0.01,
    );
}

#[test]
fn golden_cylinder() {
    let mut m = BRepModel::new();
    let s = cyl(&mut m, Point3::ZERO, 10.0, 20.0);
    assert_golden(
        &mut m,
        s,
        "cylinder r10 h20",
        &OperationKind::Primitive(
            geometry_engine::primitives::persistent_id::PrimitiveKind::Cylinder,
        ),
        std::f64::consts::PI * 100.0 * 20.0,
        0.02,
    );
}

#[test]
fn golden_bored_plate() {
    let mut m = BRepModel::new();
    let plate = boxs(&mut m, 80.0, 80.0, 16.0);
    let bore = cyl(&mut m, Point3::new(0.0, 0.0, -10.0), 12.0, 36.0);
    let holed = boolean_operation(
        &mut m,
        plate,
        bore,
        BooleanOp::Difference,
        BooleanOptions::default(),
    )
    .expect("difference");
    assert_golden(
        &mut m,
        holed,
        "bored plate 80x80x16 - r12 bore",
        &OperationKind::Boolean,
        80.0 * 80.0 * 16.0 - std::f64::consts::PI * 144.0 * 16.0,
        0.02,
    );
}

#[test]
fn golden_nurbs_loft_barrel() {
    let mut m = BRepModel::new();
    let ring = |r: f64, z: f64| {
        (0..20)
            .map(|i| {
                let a = i as f64 * std::f64::consts::TAU / 20.0;
                Point3::new(r * a.cos(), r * a.sin(), z)
            })
            .collect::<Vec<_>>()
    };
    let sections = vec![
        ring(2.0, 0.0),
        ring(3.0, 1.5),
        ring(3.5, 3.0),
        ring(3.0, 4.5),
        ring(2.0, 6.0),
    ];
    let s = nurbs_loft(&mut m, sections, NurbsLoftOptions::default()).expect("nurbs_loft");
    // A skinned barrel — provenance must say designed NurbsLoft and certify sound.
    let gt = m.ground_truth(s).expect("gt");
    assert_eq!(
        gt.provenance.as_ref().unwrap().created_by,
        OperationKind::NurbsLoft
    );
    assert!(
        gt.certificate.is_sound(),
        "barrel not sound: {}",
        gt.summary()
    );
    let c = full_contract(&mut m, s, 0.05, 0.05);
    // Freeform loft volume isn't pinned analytically; assert the topology layers.
    assert!(
        c.brep_clean && c.volume_watertight,
        "barrel contract: {:?}",
        c.failures()
    );
}
