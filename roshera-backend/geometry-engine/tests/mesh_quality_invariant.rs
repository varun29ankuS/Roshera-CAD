//! HARNESS GATE: the `MeshQuality` certificate dimension — the render mesh against
//! the CAD tessellation rules (boundary conformance, normal deviation, plus the
//! aspect/min-angle readout).
//!
//! Two contracts:
//!  1. The boundary-conformance primitive (`periodic_coverage`) flags a facet that
//!     BRIDGES across a periodic/closed lateral (the diameter-spanning "wing")
//!     and passes a thin seam-straddling facet.
//!  2. Clean primitives certify `mesh_quality.clean == true` and stay `sound` —
//!     the gate keys only on true mesh-topology defects (bridge/fold), NOT on the
//!     tall slivers a faithful tessellation routinely carries, so it does not
//!     false-flag a real part. (Calibrated against golden_contracts / ground_truth.)

use geometry_engine::harness::watertight::periodic_coverage;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use std::f64::consts::TAU;

#[test]
fn periodic_coverage_flags_bridge_and_passes_seam() {
    // A facet whose three u sit at 0, 2.0, 4.0 on a 2π circle spans most of the
    // ring (largest gap is the 2.28 wrap, coverage ≈ 4.0) — a bridge across the
    // interior. Coverage clears half the period.
    let bridge = periodic_coverage(0.0, 2.0, 4.0, TAU);
    assert!(
        bridge > TAU * 0.5,
        "a ring-spanning facet must cover > half the period, got {bridge}"
    );

    // A thin facet straddling the seam (u ≈ 0.05 and u ≈ 2π − 0.05, third at
    // 0.1) covers a tiny arc the SHORT way — it must NOT be flagged.
    let seam = periodic_coverage(0.05, TAU - 0.05, 0.1, TAU);
    assert!(
        seam < TAU * 0.5,
        "a thin seam-straddling facet must cover < half the period, got {seam}"
    );

    // A well-shaped interior facet covering a small contiguous arc.
    let small = periodic_coverage(1.0, 1.05, 1.1, TAU);
    assert!(small < 0.2, "a small-arc facet covers little, got {small}");
}

fn sid(g: GeometryId) -> geometry_engine::primitives::solid::SolidId {
    match g {
        GeometryId::Solid(id) => id,
        other => panic!("expected solid, got {other:?}"),
    }
}

#[test]
fn clean_primitives_certify_mesh_quality_clean_and_sound() {
    // Box, cylinder, sphere: faithful meshes carry tall slivers but NO bridging /
    // folded facets, so the mesh-quality gate must pass them (no false positive)
    // and they must stay sound.
    let mut m = BRepModel::new();
    let cyl = sid(TopologyBuilder::new(&mut m)
        .create_cylinder_3d(Point3::new(0.0, 0.0, 0.0), Vector3::Z, 20.0, 60.0)
        .expect("cylinder"));
    let cert = m.certify_solid(cyl);
    assert!(
        cert.mesh_quality.clean,
        "a clean cylinder must pass mesh-quality (no bridge/fold): {:?}",
        cert.mesh_quality
    );
    assert_eq!(
        cert.mesh_quality.boundary_crossing_facets, 0,
        "a clean cylinder has no boundary-crossing facets"
    );
    assert!(
        cert.mesh_quality.max_normal_deviation_deg
            <= geometry_engine::primitives::provenance::MeshQuality::MAX_NORMAL_DEVIATION_DEG,
        "a clean cylinder's facets stay on-surface: {:?}",
        cert.mesh_quality
    );
    // The shape readout is populated (the optimisation oracle).
    assert!(
        cert.mesh_quality.worst_aspect_ratio >= 1.0,
        "aspect-ratio readout must be computed"
    );
    assert!(cert.is_sound(), "a clean cylinder must be sound");

    let bx = sid(TopologyBuilder::new(&mut m)
        .create_box_3d(40.0, 40.0, 40.0)
        .expect("box"));
    assert!(
        m.certify_solid(bx).mesh_quality.clean,
        "a clean box must pass mesh-quality"
    );

    let sph = sid(TopologyBuilder::new(&mut m)
        .create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 25.0)
        .expect("sphere"));
    assert!(
        m.certify_solid(sph).mesh_quality.clean,
        "a clean sphere must pass mesh-quality"
    );
}
