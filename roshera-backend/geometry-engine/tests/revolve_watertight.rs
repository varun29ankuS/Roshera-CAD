// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! REVOLVE-TESS watertightness gate (task #63).
//!
//! `revolve_profile` made a valid B-Rep but a NON-watertight MESH for any
//! profile with SLOPED (cone) bands: the wedge's two meridian arcs sit at
//! different radii, so the chord-driven edge cache sampled them with unequal
//! counts, the structured Coons grid declined, and the curved-CDT fallback
//! choked on the thin 3D sliver → the band emitted no triangles → holes that
//! scaled with tessellation density (a revolved nozzle rendered as nothing).
//!
//! FIXED by triangulating the wedge in (u,v) PARAMETER space (well-conditioned
//! regardless of radii) from the EXACT boundary cache samples — watertight for
//! any profile shape. This sweeps the edge cases: vertical / sloped / mixed /
//! stepped walls, coarse and fine angular resolution, coarse and fine chord
//! tolerance, and a partial-angle revolution (which also builds end caps).
use geometry_engine::harness::watertight::manifold_report;
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::topology_builder::BRepModel;
use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

/// Revolve a closed (r, z) meridian profile and assert the result is a valid,
/// watertight solid at several chord tolerances.
fn assert_revolve_watertight(pts: &[(f64, f64)], segments: u32, angle_deg: f64, label: &str) {
    let mut m = BRepModel::new();
    let verts: Vec<_> = pts
        .iter()
        .map(|(r, z)| m.vertices.add(*r, 0.0, *z))
        .collect();
    let mut edges = Vec::new();
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        let line = Line::new(
            Point3::new(pts[i].0, 0.0, pts[i].1),
            Point3::new(pts[j].0, 0.0, pts[j].1),
        );
        let cid = m.curves.add(Box::new(line));
        edges.push(m.edges.add(Edge::new(
            0,
            verts[i],
            verts[j],
            cid,
            EdgeOrientation::Forward,
            ParameterRange::new(0.0, 1.0),
        )));
    }
    let opts = RevolveOptions {
        axis_origin: Point3::ZERO,
        axis_direction: Vector3::Z,
        angle: angle_deg.to_radians(),
        segments,
        ..Default::default()
    };
    let s = revolve_profile(&mut m, edges, opts)
        .unwrap_or_else(|e| panic!("{label}: revolve_profile failed: {e:?}"));

    let v = validate_solid_scoped(&m, s, Tolerance::default(), ValidationLevel::Standard);
    assert!(v.is_valid, "{label}: B-Rep invalid: {:?}", v.errors);

    for defl in [0.5_f64, 0.1, 0.02] {
        let rep = manifold_report(&m, s, defl, 1e-6)
            .unwrap_or_else(|| panic!("{label}: manifold_report none @defl {defl}"));
        assert_eq!(
            (rep.boundary_edges, rep.nonmanifold_edges),
            (0, 0),
            "{label}: not watertight @defl {defl} (open={}, nm={})",
            rep.boundary_edges,
            rep.nonmanifold_edges
        );
    }
}

#[test]
fn revolve_vertical_tube_watertight_63() {
    // Constant-radius walls — the case that already worked (regression guard).
    assert_revolve_watertight(
        &[(10.0, 0.0), (10.0, 20.0), (6.0, 20.0), (6.0, 0.0)],
        32,
        360.0,
        "vertical tube",
    );
}

#[test]
fn revolve_cone_shell_watertight_63() {
    // Both walls sloped (the bug): inward-tapering cone shell.
    assert_revolve_watertight(
        &[(10.0, 0.0), (4.0, 20.0), (2.0, 20.0), (8.0, 0.0)],
        48,
        360.0,
        "cone shell",
    );
}

#[test]
fn revolve_frustum_tube_watertight_63() {
    // Single-slope frustum tube, both rims different radii.
    assert_revolve_watertight(
        &[(12.0, 0.0), (7.0, 25.0), (5.0, 25.0), (10.0, 0.0)],
        24,
        360.0,
        "frustum tube",
    );
}

#[test]
fn revolve_stepped_tube_watertight_63() {
    // Mixed vertical + horizontal steps (cylinder/disc bands interleaved).
    assert_revolve_watertight(
        &[
            (10.0, 0.0),
            (10.0, 10.0),
            (7.0, 10.0),
            (7.0, 20.0),
            (4.0, 20.0),
            (4.0, 0.0),
        ],
        40,
        360.0,
        "stepped tube",
    );
}

#[test]
fn revolve_de_laval_engine_watertight_63() {
    // The hollow rocket engine: chamber wall, convergent + divergent cone
    // walls, inner gas-path contour — vertical, sloped and horizontal bands
    // mixed in one profile. This is the part that rendered as nothing.
    assert_revolve_watertight(
        &[
            (18.0, -30.0),
            (6.0, -12.0),
            (20.0, 0.0),
            (20.0, 40.0),
            (16.0, 40.0),
            (16.0, 0.0),
            (4.0, -12.0),
            (16.0, -30.0),
        ],
        64,
        360.0,
        "de Laval engine",
    );
}

#[test]
fn revolve_coarse_and_fine_segments_watertight_63() {
    // Angular resolution extremes on a sloped profile.
    let cone = [(10.0, 0.0), (4.0, 20.0), (2.0, 20.0), (8.0, 0.0)];
    assert_revolve_watertight(&cone, 6, 360.0, "cone coarse(6)");
    assert_revolve_watertight(&cone, 120, 360.0, "cone fine(120)");
}

#[test]
fn revolve_partial_angle_cone_watertight_63() {
    // Partial revolution exercises the start/end cap path on a sloped profile.
    assert_revolve_watertight(
        &[(10.0, 0.0), (4.0, 20.0), (2.0, 20.0), (8.0, 0.0)],
        32,
        180.0,
        "cone 180°",
    );
}
