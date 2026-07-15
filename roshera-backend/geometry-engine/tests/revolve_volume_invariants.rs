// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Volume invariants for the revolve operation, via Pappus's theorem.
//!
//! Revolving a planar region of area A about an external axis, through angle θ,
//! sweeps a solid of volume `θ · R · A`, where R is the distance from the axis
//! to the region's centroid. We revolve axis-parallel rectangles in the XZ
//! plane (x ∈ [x0,x1], z ∈ [z0,z1]) about the Z axis: A = (x1−x0)(z1−z0),
//! R = (x0+x1)/2, so the expected volume is θ·R·A = (θ/2)(x1²−x0²)(z1−z0).
//! (Full 2π gives the annular ring π(x1²−x0²)(z1−z0).)
//!
//! The revolved lateral surfaces are curved, so volumes come from the
//! tessellated pipeline — asserted at 5%. A revolve that returns a typed Err
//! (kernel numerical-rigor contract) is treated as a skip.

use std::f64::consts::PI;

use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams, TriangleMesh};

// Divergence-theorem volume of the WATERTIGHT tessellated solid. NOTE: this is
// used instead of `mass_properties_for`, whose volume path disagrees with the
// tessellation for revolved (generic SurfaceOfRevolution) solids — a separate
// documented residual. Both partial and full revolutions now tessellate
// watertight (the wall wedges are structured-grid tessellated — REVOLVE-
// ROBUSTNESS #47), so this divergence volume is exact across the full range.
fn mesh_volume(mesh: &TriangleMesh) -> f64 {
    let mut v = 0.0;
    for t in &mesh.triangles {
        let a = mesh.vertices[t[0] as usize].position;
        let b = mesh.vertices[t[1] as usize].position;
        let c = mesh.vertices[t[2] as usize].position;
        v += (a.x * (b.y * c.z - b.z * c.y) - a.y * (b.x * c.z - b.z * c.x)
            + a.z * (b.x * c.y - b.y * c.x))
            / 6.0;
    }
    v.abs()
}

fn add_line_edge(model: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let s = model.vertices.get(a).expect("v").position;
    let e = model.vertices.get(b).expect("v").position;
    let curve_id = model
        .curves
        .add(Box::new(Line::new(Point3::from(s), Point3::from(e))));
    model.edges.add(Edge::new_auto_range(
        0,
        a,
        b,
        curve_id,
        EdgeOrientation::Forward,
    ))
}

/// Closed CCW rectangle in the XZ plane (y = 0), x ∈ [x0,x1], z ∈ [z0,z1].
fn offset_rectangle(model: &mut BRepModel, x0: f64, x1: f64, z0: f64, z1: f64) -> Vec<EdgeId> {
    let v0 = model.vertices.add(x0, 0.0, z0);
    let v1 = model.vertices.add(x1, 0.0, z0);
    let v2 = model.vertices.add(x1, 0.0, z1);
    let v3 = model.vertices.add(x0, 0.0, z1);
    vec![
        add_line_edge(model, v0, v1),
        add_line_edge(model, v1, v2),
        add_line_edge(model, v2, v3),
        add_line_edge(model, v3, v0),
    ]
}

/// Revolve the rectangle about the Z axis by `angle`; return Some(volume) on
/// success.
fn revolve_volume(x0: f64, x1: f64, z0: f64, z1: f64, angle: f64) -> Option<f64> {
    let mut model = BRepModel::new();
    let edges = offset_rectangle(&mut model, x0, x1, z0, z1);
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle,
        ..RevolveOptions::default()
    };
    let solid_id = revolve_profile(&mut model, edges, opts).ok()?;
    // HARNESS-AS-DEFAULT: a successful revolve must be a VALID, WATERTIGHT B-Rep,
    // not merely volume-correct. (Mesh-independent validity + mesh manifoldness.)
    let v = validate_solid_scoped(
        &model,
        solid_id,
        Tolerance::default(),
        ValidationLevel::Standard,
    );
    assert!(
        v.is_valid,
        "revolve [{x0},{x1}]x[{z0},{z1}] @ {angle:.4}: invalid B-Rep: {:?}",
        v.errors
    );
    if let Some(rep) = manifold_report(&model, solid_id, 0.5, 1e-6) {
        assert_eq!(
            (rep.boundary_edges, rep.nonmanifold_edges),
            (0, 0),
            "revolve [{x0},{x1}]x[{z0},{z1}] @ {angle:.4}: not watertight (open={} nm={})",
            rep.boundary_edges,
            rep.nonmanifold_edges
        );
    }
    let solid = model.solids.get(solid_id)?;
    let mesh = tessellate_solid(solid, &model, &TessellationParams::default());
    Some(mesh_volume(&mesh))
}

fn pappus_volume(x0: f64, x1: f64, z0: f64, z1: f64, angle: f64) -> f64 {
    0.5 * angle * (x1 * x1 - x0 * x0) * (z1 - z0)
}

fn rel_close(a: f64, b: f64, tol: f64) -> bool {
    if b == 0.0 {
        a.abs() <= tol
    } else {
        ((a - b) / b).abs() <= tol
    }
}

// =====================================================================
// Full 360° revolve → annular ring volume = π(x1²−x0²)(z1−z0).
// =====================================================================

// Full 360° revolves now tessellate watertight: the per-segment wall wedges are
// tessellated as structured grids (REVOLVE-ROBUSTNESS #47 fixed), so the
// divergence volume matches Pappus exactly. A `None` here would mean revolve
// regressed into an outright failure and must fail the test, not skip it.
macro_rules! full_revolve_test {
    ($name:ident, $x0:expr, $x1:expr, $z0:expr, $z1:expr) => {
        #[test]
        fn $name() {
            let vol = revolve_volume($x0, $x1, $z0, $z1, 2.0 * PI)
                .expect("full revolve of a known-good profile must tessellate");
            let expected = pappus_volume($x0, $x1, $z0, $z1, 2.0 * PI);
            assert!(
                rel_close(vol, expected, 0.05),
                "full revolve [{},{}]x[{},{}]: vol {} vs Pappus {}",
                $x0,
                $x1,
                $z0,
                $z1,
                vol,
                expected
            );
        }
    };
}

full_revolve_test!(full_ring_2_4_0_1, 2.0, 4.0, 0.0, 1.0);
full_revolve_test!(full_ring_1_3_0_2, 1.0, 3.0, 0.0, 2.0);
full_revolve_test!(full_ring_5_6_0_3, 5.0, 6.0, 0.0, 3.0);
full_revolve_test!(full_ring_2_5_1_2, 2.0, 5.0, 1.0, 2.0);
full_revolve_test!(full_ring_3_4_0_5, 3.0, 4.0, 0.0, 5.0);
full_revolve_test!(full_ring_1_2_0_1, 1.0, 2.0, 0.0, 1.0);

// =====================================================================
// Partial revolve → θ·R·A.
// =====================================================================

macro_rules! partial_revolve_test {
    ($name:ident, $x0:expr, $x1:expr, $z0:expr, $z1:expr, $angle:expr) => {
        #[test]
        fn $name() {
            // Partial revolves (≤180°) tessellate watertight and MUST succeed
            // for these fixed known-good profiles — a `None` here means revolve
            // regressed into an outright failure, which must fail the test
            // rather than silently skip it (the vacuous-pass trap).
            let vol = revolve_volume($x0, $x1, $z0, $z1, $angle)
                .expect("partial revolve (≤180°) of a known-good profile must succeed");
            let expected = pappus_volume($x0, $x1, $z0, $z1, $angle);
            assert!(
                rel_close(vol, expected, 0.06),
                "revolve angle {}: vol {} vs Pappus {}",
                $angle,
                vol,
                expected
            );
        }
    };
}

partial_revolve_test!(half_2_4_0_1, 2.0, 4.0, 0.0, 1.0, std::f64::consts::PI);
partial_revolve_test!(
    quarter_2_4_0_1,
    2.0,
    4.0,
    0.0,
    1.0,
    std::f64::consts::FRAC_PI_2
);
// 270° (> 180°): the past-π wall wedges now tessellate as structured grids
// (REVOLVE-ROBUSTNESS #47), so this is watertight and exact like the ≤180° cases.
partial_revolve_test!(
    three_quarter_2_4_0_1,
    2.0,
    4.0,
    0.0,
    1.0,
    3.0 * std::f64::consts::FRAC_PI_2
);
partial_revolve_test!(half_1_3_0_2, 1.0, 3.0, 0.0, 2.0, std::f64::consts::PI);
partial_revolve_test!(
    third_3_5_0_2,
    3.0,
    5.0,
    0.0,
    2.0,
    2.0 * std::f64::consts::PI / 3.0
);

#[test]
fn revolve_volume_is_monotone_in_angle() {
    let small = revolve_volume(2.0, 4.0, 0.0, 1.0, PI / 2.0);
    let large = revolve_volume(2.0, 4.0, 0.0, 1.0, PI);
    if let (Some(s), Some(l)) = (small, large) {
        assert!(
            l > s,
            "larger sweep angle must give larger volume: {l} !> {s}"
        );
    }
}

#[test]
fn revolve_volume_scales_with_centroid_radius() {
    // Same area & angle, larger centroid radius ⇒ larger swept volume.
    // Uses a PARTIAL sweep (watertight tessellation) — a full sweep would hit
    // the documented full-revolution tessellation residual.
    let near = revolve_volume(1.0, 2.0, 0.0, 1.0, PI); // R = 1.5
    let far = revolve_volume(5.0, 6.0, 0.0, 1.0, PI); // R = 5.5
    if let (Some(n), Some(f)) = (near, far) {
        assert!(
            f > n,
            "larger centroid radius must give larger volume: {f} !> {n}"
        );
    }
}

use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    #[test]
    fn prop_full_revolve_matches_pappus(
        x0 in 0.5f64..6.0, width in 0.5f64..3.0,
        z0 in 0.0f64..3.0, height in 0.5f64..3.0,
    ) {
        let (x1, z1) = (x0 + width, z0 + height);
        let vol = revolve_volume(x0, x1, z0, z1, 2.0 * PI)
            .expect("full revolve must tessellate");
        let expected = pappus_volume(x0, x1, z0, z1, 2.0 * PI);
        prop_assert!(rel_close(vol, expected, 0.05), "{vol} vs {expected}");
    }
}
