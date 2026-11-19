// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Section hatching is a PAPER convention (ISO 128-50): the same pitch at
//! every scale. The spacing used to be fixed in model units, so a 1:5 section
//! printed its hatch 0.57 mm apart and a 2:1 section 5.7 mm apart.
//!
//! Mutation: make `hatch_spacing` return its 1:1 value at every scale -> the
//! 1:5 and 2:1 cases read 0.6 and 6.0 mm and fail.

use geometry_engine::drawing::section_view;
use geometry_engine::drawing::section_view::HATCH_PITCH_PAPER_MM;
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};

/// Printed hatch pitch (square to the lines), mm, of a section of a
/// 120 × 80 × 60 box cut through its middle and drawn at `scale`.
fn printed_pitch(scale: f64) -> f64 {
    let mut m = BRepModel::new();
    let s = match TopologyBuilder::new(&mut m).create_box_3d(120.0, 80.0, 60.0) {
        Ok(GeometryId::Solid(s)) => s,
        other => panic!("box: {other:?}"),
    };
    let view = section_view(
        &m,
        s,
        uuid::Uuid::nil(),
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 1.0, 0.0),
        "SECTION A-A",
        [0.0, 0.0],
        scale,
    )
    .expect("the plane cuts the box");
    // Each hatch line is y = x + c in view space.
    let mut cs: Vec<f64> = view
        .hatch_polylines
        .iter()
        .map(|p| p.points[0][1] - p.points[0][0])
        .collect();
    cs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    cs.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    assert!(
        cs.len() >= 3,
        "a section this size carries several hatch lines"
    );
    let step = cs
        .windows(2)
        .map(|w| w[1] - w[0])
        .fold(f64::INFINITY, f64::min);
    step / std::f64::consts::SQRT_2 * scale
}

#[test]
fn hatch_prints_at_the_same_pitch_at_every_scale() {
    for scale in [0.2, 0.5, 1.0, 2.0] {
        let pitch = printed_pitch(scale);
        assert!(
            (pitch - HATCH_PITCH_PAPER_MM).abs() < 1e-6,
            "at scale {scale} the hatch prints {pitch:.3} mm apart, not {HATCH_PITCH_PAPER_MM} mm"
        );
    }
}
