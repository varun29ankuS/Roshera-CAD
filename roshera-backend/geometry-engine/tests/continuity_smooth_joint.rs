// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Two patches whose normals agree along their shared edge join at least
//! tangentially (G1). The angle between the normals was computed as
//! `acos(1 - |n1.n2|)` -- for parallel normals acos(0) = 90 degrees -- so every
//! smooth joint was classified as a mere positional (G0) one, and the
//! tangent error read ~90 degrees. It is `acos(|n1.n2|)`.

use geometry_engine::math::continuity_analysis::{
    analyze_surface_continuity, ContinuityType, SurfaceEdge,
};
use geometry_engine::math::nurbs::NurbsSurface;
use geometry_engine::math::{Point3, Tolerance};

/// A flat bilinear patch over x in [x0, x1], y in [0, 1], at z = 0.
fn flat_patch(x0: f64, x1: f64) -> NurbsSurface {
    NurbsSurface::new(
        vec![
            vec![Point3::new(x0, 0.0, 0.0), Point3::new(x0, 1.0, 0.0)],
            vec![Point3::new(x1, 0.0, 0.0), Point3::new(x1, 1.0, 0.0)],
        ],
        vec![vec![1.0, 1.0], vec![1.0, 1.0]],
        vec![0.0, 0.0, 1.0, 1.0],
        vec![0.0, 0.0, 1.0, 1.0],
        1,
        1,
    )
    .expect("patch")
}

#[test]
fn two_flat_patches_meeting_in_a_line_join_smoothly() {
    let a = flat_patch(0.0, 1.0);
    let b = flat_patch(1.0, 2.0);
    // a's u = 1 edge is b's u = 0 edge.
    let r = analyze_surface_continuity(
        &a,
        SurfaceEdge {
            edge_type: 1,
            range: (0.0, 1.0),
        },
        &b,
        SurfaceEdge {
            edge_type: 0,
            range: (0.0, 1.0),
        },
        8,
        Tolerance::default(),
    )
    .expect("analyses");
    assert!(
        r.continuity >= ContinuityType::G1,
        "coplanar patches classified {:?} ({})",
        r.continuity,
        r.message
    );
    assert!(
        r.tangent_error.unwrap_or(f64::INFINITY) < 1e-9,
        "tangent error {:?} for parallel normals",
        r.tangent_error
    );
}
