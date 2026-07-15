// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Revolve must produce a VALID, watertight-topology B-Rep (not the previous
//! island-per-quad non-manifold shell). `revolve_profile` validates its result
//! by default, so a successful return already certifies a manifold solid.
//!
//! Volume is NOT asserted here. Two separate, pre-existing downstream residuals
//! (documented, tracked) make revolved-solid volume unreliable today and are
//! independent of the B-Rep construction this fix corrected:
//!   * full-revolution tessellation: the per-segment `SurfaceOfRevolution`
//!     patches are not trimmed to their angular wedge by the tessellator, so
//!     `tessellate_solid` leaves boundary-edge gaps (CDT-γ-class follow-up);
//!   * `mass_properties_for` uses a volume path that disagrees with a watertight
//!     `tessellate_solid` divergence volume on revolved solids.
//! Both are tracked separately; this file pins only the topology fix.

use std::f64::consts::{PI, TAU};

use geometry_engine::math::{Point3, Tolerance};
use geometry_engine::operations::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::Line;
use geometry_engine::primitives::edge::{Edge, EdgeId, EdgeOrientation};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_model_enhanced, ValidationLevel};

fn add_line_edge(m: &mut BRepModel, a: u32, b: u32) -> EdgeId {
    let pa = m.vertices.get(a).expect("a").position;
    let pb = m.vertices.get(b).expect("b").position;
    let cid = m
        .curves
        .add(Box::new(Line::new(Point3::from(pa), Point3::from(pb))));
    m.edges
        .add(Edge::new_auto_range(0, a, b, cid, EdgeOrientation::Forward))
}

/// CCW rectangle in the XZ plane, x ∈ [x0,x1], z ∈ [z0,z1] (offset from the axis).
fn rect(m: &mut BRepModel, x0: f64, x1: f64, z0: f64, z1: f64) -> Vec<EdgeId> {
    let v0 = m.vertices.add(x0, 0.0, z0);
    let v1 = m.vertices.add(x1, 0.0, z0);
    let v2 = m.vertices.add(x1, 0.0, z1);
    let v3 = m.vertices.add(x0, 0.0, z1);
    vec![
        add_line_edge(m, v0, v1),
        add_line_edge(m, v1, v2),
        add_line_edge(m, v2, v3),
        add_line_edge(m, v3, v0),
    ]
}

#[test]
fn full_revolution_is_a_valid_manifold() {
    // revolve_profile validates the result (validate_result defaults true), so
    // Ok ⇒ the B-Rep is a valid closed manifold.
    for &(x0, x1) in &[(2.0, 4.0), (20.0, 30.0), (0.5, 1.0)] {
        let mut m = BRepModel::new();
        let edges = rect(&mut m, x0, x1, 0.0, 3.0);
        let r = revolve_profile(
            &mut m,
            edges,
            RevolveOptions {
                angle: TAU,
                segments: 24,
                cap_ends: false,
                ..Default::default()
            },
        );
        assert!(
            r.is_ok(),
            "full revolve R={x0}-{x1} not a valid manifold: {r:?}"
        );
    }
}

#[test]
fn partial_revolution_is_valid_with_caps() {
    let mut m = BRepModel::new();
    let edges = rect(&mut m, 2.0, 4.0, 0.0, 1.0);
    let r = revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: PI,
            segments: 24,
            cap_ends: true,
            ..Default::default()
        },
    );
    assert!(r.is_ok(), "partial revolve not valid: {r:?}");
}

#[test]
fn revolved_solid_validates_as_whole_model() {
    // The scratch profile face is removed, so the whole model (not just the
    // result solid) is a valid manifold — no orphaned single-use edges.
    let mut m = BRepModel::new();
    let edges = rect(&mut m, 2.0, 4.0, 0.0, 1.0);
    revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: TAU,
            segments: 16,
            cap_ends: false,
            ..Default::default()
        },
    )
    .expect("revolve");
    let r = validate_model_enhanced(&m, Tolerance::default(), ValidationLevel::Standard);
    assert!(
        r.is_valid,
        "model invalid after revolve ({} errors): {:?}",
        r.errors.len(),
        r.errors.iter().take(3).collect::<Vec<_>>()
    );
}

#[test]
fn revolve_segments_below_minimum_still_valid() {
    // segments is clamped up internally; a coarse revolution is still a valid
    // manifold.
    let mut m = BRepModel::new();
    let edges = rect(&mut m, 3.0, 5.0, 0.0, 2.0);
    let r = revolve_profile(
        &mut m,
        edges,
        RevolveOptions {
            angle: TAU,
            segments: 3,
            cap_ends: false,
            ..Default::default()
        },
    );
    assert!(r.is_ok(), "coarse full revolve not valid: {r:?}");
}
