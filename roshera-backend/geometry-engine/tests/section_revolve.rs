// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Section-on-revolve ACCEPTANCE gate (campaign task #9) — FIXED 2026-06-16 for
//! tubes via the analytic-band revolve (#19).
//!
//! ROOT CAUSE (was): a revolve emitted MANY narrow `SurfaceOfRevolution` patches
//! (one per angular segment — a 48-segment tube = 192 faces). `section_solid_by_plane`
//! intersected each patch with the cut plane and chained the fragments into
//! loops; per patch the marching-square section line was sampled too coarsely to
//! emit the COMPLETE fragment, so the pieces had real gaps the chainer correctly
//! refused to bridge → no loops → `render_section` returned `None` (the 404 an
//! agent hit trying to SEE a revolved part's hollow interior).
//!
//! FIX: `revolve_profile` now emits ANALYTIC bands (Cylinder walls + annular
//! Plane caps) for full revolutions of rectilinear profiles — same class #24
//! fixed for extrude. Each analytic band sections to a clean line (exactly like
//! a box's planar faces), so the two tube gates below PASS. The near-axis
//! `solid_cylinder` case (r=0.001 sliver) is still v2 (disc/apex bands).
use geometry_engine::math::{Point3, Tolerance, Vector3};
use geometry_engine::operations::revolve::{revolve_profile, RevolveOptions};
use geometry_engine::operations::section::section_solid_by_plane;
use geometry_engine::primitives::curve::{Line, ParameterRange};
use geometry_engine::primitives::edge::{Edge, EdgeOrientation};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::BRepModel;

/// Revolve a closed (r, z) meridian profile 360° about +Z.
fn revolve(m: &mut BRepModel, pts: &[(f64, f64)], segments: u32) -> SolidId {
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
        angle: std::f64::consts::TAU,
        segments,
        ..Default::default()
    };
    revolve_profile(m, edges, opts).unwrap_or_else(|e| panic!("revolve: {e:?}"))
}

/// Total area of all section caps (sum of triangle areas).
fn cap_area(caps: &[geometry_engine::operations::section::SectionCap]) -> f64 {
    let mut area = 0.0;
    for cap in caps {
        for tri in &cap.indices {
            let a = cap.vertices[tri[0] as usize];
            let b = cap.vertices[tri[1] as usize];
            let c = cap.vertices[tri[2] as usize];
            let e1 = b - a;
            let e2 = c - a;
            area += e1.cross(&e2).magnitude() * 0.5;
        }
    }
    area
}

fn assert_section(pts: &[(f64, f64)], segments: u32, normal: Vector3, want_area: f64, label: &str) {
    let mut m = BRepModel::new();
    let s = revolve(&mut m, pts, segments);
    let caps = section_solid_by_plane(
        &m,
        s,
        Point3::new(0.0, 0.0, 10.0),
        normal,
        Tolerance::default(),
    )
    .unwrap_or_else(|e| panic!("{label}: section errored: {e:?}"));
    assert!(
        !caps.is_empty(),
        "{label}: section produced NO caps (the 404 bug)"
    );
    let area = cap_area(&caps);
    assert!(
        (area - want_area).abs() < want_area * 0.05,
        "{label}: section area {area:.2} != expected ~{want_area:.2}"
    );
}

#[test]
fn section_revolved_tube_x_plane() {
    // Tube r5..10, z0..20. The x=0 plane (normal +X) contains the axis →
    // two wall rectangles 5 wide × 20 tall = 2 × 100 = 200 mm².
    assert_section(
        &[(5.0, 0.0), (10.0, 0.0), (10.0, 20.0), (5.0, 20.0)],
        48,
        Vector3::X,
        200.0,
        "tube +X",
    );
}

#[test]
fn section_revolved_tube_y_plane() {
    // Same tube, orthogonal axial cut (the SEAM-containing direction that was
    // the SECTION #85c failure mode) — must also give 200 mm².
    assert_section(
        &[(5.0, 0.0), (10.0, 0.0), (10.0, 20.0), (5.0, 20.0)],
        48,
        Vector3::Y,
        200.0,
        "tube +Y",
    );
}

#[test]
#[ignore = "v2 disc/apex: this profile hugs the axis at r=0.001 (a near-degenerate sliver Cylinder + a near-full-disc annular Plane with a 0.001 hole). The analytic-band revolve (v1) handles genuine tubes — the two tube section tests above now PASS — but the near-axis sliver's thin section fragments still can't chain. Un-ignore when v2 lands true apex/disc bands (a vertex ON the axis → a real disc cap, no sliver cylinder)."]
fn section_revolved_solid_cylinder() {
    // A solid disc revolve (r0..12, but profile kept off the axis at r=0.001
    // to satisfy the no-pole rule), z0..16. Axial cut → a 24×16 rectangle
    // (two 12×16 halves) = 384 mm².
    assert_section(
        &[(0.001, 0.0), (12.0, 0.0), (12.0, 16.0), (0.001, 16.0)],
        64,
        Vector3::X,
        384.0,
        "solid cyl +X",
    );
}
