//! Slice α tests — `primitives::curve::Polyline` composite curve.
//!
//! These cover the full `Curve` trait surface (evaluate, derivatives,
//! arc length, closest point, plane fit, split, subcurve, intersection,
//! offset, transform, reversed, to_nurbs, …) for the piecewise-linear
//! curve that backs sketched polyline edges after Slice α.
//!
//! The motivating use case is the sketch handler in
//! `roshera-backend/api-server/src/sketch.rs::build_loop_edges`: every
//! polyline-tool shape registers ONE `Polyline` covering the whole loop
//! and references it from N per-segment edges, each with its own
//! parameter sub-range `[i/N, (i+1)/N]`. That sharing means the
//! frontend hover/pick path (`sample_edge_polyline` in
//! `protocol/message_handlers.rs`) returns the full outline for any
//! constituent edge, instead of a single short segment indistinguishable
//! from a tessellation triangle border.
//!
//! See `roshera-backend/geometry-engine/src/primitives/curve.rs` for the
//! production code.

// AUDIT-H13: Reason for `#![allow(clippy::expect_used)]` — test-only file.
// `expect(...)` on fixture/scaffolding code surfaces invariant violations
// with a clear message at the failure site, which is the desired failure
// mode in tests. The workspace `expect_used = "deny"` lint targets
// production panic-freedom; test scaffolding is exempt by design.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use geometry_engine::math::tolerance::NORMAL_TOLERANCE;
use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::primitives::curve::{Continuity, Curve, Line, Polyline};
use geometry_engine::primitives::surface::Plane;

const TOL: f64 = 1e-9;

fn point(x: f64, y: f64, z: f64) -> Point3 {
    Point3::new(x, y, z)
}

/// Square loop of side 1 on z = 0, vertices (0,0)→(1,0)→(1,1)→(0,1)→(0,0).
/// Total perimeter = 4, four equal segments.
fn unit_square() -> Polyline {
    Polyline::new(vec![
        point(0.0, 0.0, 0.0),
        point(1.0, 0.0, 0.0),
        point(1.0, 1.0, 0.0),
        point(0.0, 1.0, 0.0),
        point(0.0, 0.0, 0.0),
    ])
    .expect("unit_square polyline")
}

// ─── Construction ──────────────────────────────────────────────────

#[test]
fn rejects_fewer_than_two_vertices() {
    assert!(Polyline::new(vec![]).is_err());
    assert!(Polyline::new(vec![point(0.0, 0.0, 0.0)]).is_err());
}

#[test]
fn accepts_two_or_more_vertices() {
    assert!(Polyline::new(vec![point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)]).is_ok());
    assert!(Polyline::new(vec![
        point(0.0, 0.0, 0.0),
        point(1.0, 0.0, 0.0),
        point(2.0, 1.0, 0.0)
    ])
    .is_ok());
}

#[test]
fn segment_count_matches_vertex_count_minus_one() {
    let sq = unit_square();
    assert_eq!(sq.segment_count(), 4);
    assert_eq!(sq.vertices.len(), 5);
}

// ─── Evaluate ──────────────────────────────────────────────────────

#[test]
fn evaluate_at_vertices_hits_them_exactly() {
    let sq = unit_square();
    // 4 segments → vertices at t = 0, 0.25, 0.5, 0.75, 1.0.
    for (i, expected) in sq.vertices.iter().enumerate() {
        let t = i as f64 / 4.0;
        let p = sq.evaluate(t).expect("evaluate").position;
        assert!(
            (p - *expected).magnitude() < TOL,
            "t={t} expected {expected:?} got {p:?}"
        );
    }
}

#[test]
fn evaluate_within_segment_is_linear() {
    let sq = unit_square();
    // Mid of segment 0 (t ∈ [0, 0.25]): expect (0.5, 0, 0).
    let p = sq.evaluate(0.125).expect("evaluate").position;
    assert!((p - point(0.5, 0.0, 0.0)).magnitude() < TOL);
    // Mid of segment 2 (t ∈ [0.5, 0.75]): expect (0.5, 1, 0).
    let p = sq.evaluate(0.625).expect("evaluate").position;
    assert!((p - point(0.5, 1.0, 0.0)).magnitude() < TOL);
}

#[test]
fn evaluate_clamps_out_of_range() {
    let sq = unit_square();
    let neg = sq.evaluate(-0.5).expect("eval clamped").position;
    let zero = sq.evaluate(0.0).expect("eval zero").position;
    assert!((neg - zero).magnitude() < TOL);

    let over = sq.evaluate(1.5).expect("eval clamped").position;
    let one = sq.evaluate(1.0).expect("eval one").position;
    assert!((over - one).magnitude() < TOL);
}

#[test]
fn first_derivative_scales_by_segment_count() {
    // 4 segments of length 1 each: each segment vector is unit-length,
    // but the chain-rule scaled derivative wrt t ∈ [0,1] is N · dir.
    let sq = unit_square();
    let cp = sq.evaluate(0.125).expect("evaluate mid-seg-0");
    // Segment 0 direction is +X with length 1, so derivative is (4, 0, 0).
    assert!((cp.derivative1 - Vector3::new(4.0, 0.0, 0.0)).magnitude() < TOL);

    let cp = sq.evaluate(0.625).expect("evaluate mid-seg-2");
    // Segment 2 direction is -X (from (1,1) to (0,1)).
    assert!((cp.derivative1 - Vector3::new(-4.0, 0.0, 0.0)).magnitude() < TOL);
}

#[test]
fn second_and_third_derivatives_are_zero() {
    let sq = unit_square();
    let cp = sq.evaluate(0.3).expect("evaluate");
    assert!(cp.derivative2.unwrap().magnitude() < TOL);
    assert!(cp.derivative3.unwrap().magnitude() < TOL);
}

// ─── Parameter range / closure ─────────────────────────────────────

#[test]
fn parameter_range_is_unit() {
    let sq = unit_square();
    let r = sq.parameter_range();
    assert!((r.start - 0.0).abs() < TOL);
    assert!((r.end - 1.0).abs() < TOL);
}

#[test]
fn closed_loop_first_eq_last_reports_closed() {
    assert!(unit_square().is_closed());
}

#[test]
fn open_polyline_reports_not_closed() {
    let p = Polyline::new(vec![
        point(0.0, 0.0, 0.0),
        point(1.0, 0.0, 0.0),
        point(1.0, 1.0, 0.0),
    ])
    .expect("open polyline");
    assert!(!p.is_closed());
}

// ─── Linearity / planarity ─────────────────────────────────────────

#[test]
fn collinear_vertices_report_linear() {
    let p = Polyline::new(vec![
        point(0.0, 0.0, 0.0),
        point(1.0, 0.0, 0.0),
        point(2.0, 0.0, 0.0),
        point(3.0, 0.0, 0.0),
    ])
    .expect("collinear polyline");
    assert!(p.is_linear(NORMAL_TOLERANCE));
}

#[test]
fn non_collinear_polyline_is_not_linear() {
    assert!(!unit_square().is_linear(NORMAL_TOLERANCE));
}

#[test]
fn planar_polyline_reports_planar() {
    assert!(unit_square().is_planar(NORMAL_TOLERANCE));
}

#[test]
fn skew_polyline_is_not_planar() {
    // Tetrahedron-like skew: four vertices not on a single plane.
    let p = Polyline::new(vec![
        point(0.0, 0.0, 0.0),
        point(1.0, 0.0, 0.0),
        point(0.0, 1.0, 0.0),
        point(0.0, 0.0, 1.0),
    ])
    .expect("skew polyline");
    assert!(!p.is_planar(NORMAL_TOLERANCE));
}

#[test]
fn get_plane_returns_plane_with_correct_normal_for_xy_square() {
    let p = unit_square().get_plane(NORMAL_TOLERANCE).expect("plane");
    // Newell's method on a CCW XY loop returns +Z (or -Z) normal.
    let n = p.normal;
    assert!(
        (n - Vector3::new(0.0, 0.0, 1.0)).magnitude() < 1e-6
            || (n - Vector3::new(0.0, 0.0, -1.0)).magnitude() < 1e-6,
        "expected ±Z normal, got {:?}",
        n
    );
}

// ─── Arc length ────────────────────────────────────────────────────

#[test]
fn arc_length_full_range_is_total_perimeter() {
    let sq = unit_square();
    let len = sq
        .arc_length_between(0.0, 1.0, NORMAL_TOLERANCE)
        .expect("arc length");
    assert!((len - 4.0).abs() < TOL, "len = {len}");
}

#[test]
fn arc_length_single_segment_is_one() {
    let sq = unit_square();
    // t ∈ [0, 0.25] covers segment 0 entirely.
    let len = sq
        .arc_length_between(0.0, 0.25, NORMAL_TOLERANCE)
        .expect("arc length");
    assert!((len - 1.0).abs() < TOL);
}

#[test]
fn arc_length_partial_segment() {
    let sq = unit_square();
    // t ∈ [0, 0.125] covers half of segment 0 → length 0.5.
    let len = sq
        .arc_length_between(0.0, 0.125, NORMAL_TOLERANCE)
        .expect("arc length");
    assert!((len - 0.5).abs() < TOL);
}

#[test]
fn arc_length_spans_multiple_segments() {
    let sq = unit_square();
    // t ∈ [0.125, 0.625]: 0.5 of seg 0 + seg 1 + 0.5 of seg 2 = 2.0.
    let len = sq
        .arc_length_between(0.125, 0.625, NORMAL_TOLERANCE)
        .expect("arc length");
    assert!((len - 2.0).abs() < TOL);
}

#[test]
fn arc_length_handles_swapped_args() {
    let sq = unit_square();
    let a = sq.arc_length_between(0.2, 0.6, NORMAL_TOLERANCE).unwrap();
    let b = sq.arc_length_between(0.6, 0.2, NORMAL_TOLERANCE).unwrap();
    assert!((a - b).abs() < TOL);
}

#[test]
fn parameter_at_length_inverts_arc_length() {
    let sq = unit_square();
    for target in [0.5_f64, 1.0, 1.5, 2.0, 3.5] {
        let t = sq
            .parameter_at_length(target, NORMAL_TOLERANCE)
            .expect("param at length");
        let back = sq.arc_length_between(0.0, t, NORMAL_TOLERANCE).unwrap();
        assert!(
            (back - target).abs() < 1e-6,
            "round-trip mismatch: target={target} t={t} back={back}"
        );
    }
}

// ─── Closest point ─────────────────────────────────────────────────

#[test]
fn closest_point_on_a_segment_is_the_point_itself() {
    let sq = unit_square();
    // (0.5, 0, 0) lies on segment 0 → distance 0, t = 0.125.
    let (t, q) = sq
        .closest_point(&point(0.5, 0.0, 0.0), NORMAL_TOLERANCE)
        .expect("closest");
    assert!((t - 0.125).abs() < TOL);
    assert!((q - point(0.5, 0.0, 0.0)).magnitude() < TOL);
}

#[test]
fn closest_point_to_outside_uses_perpendicular_foot() {
    let sq = unit_square();
    // (0.5, -1, 0) → perpendicular foot on segment 0 at (0.5, 0, 0).
    let (t, q) = sq
        .closest_point(&point(0.5, -1.0, 0.0), NORMAL_TOLERANCE)
        .expect("closest");
    assert!((q - point(0.5, 0.0, 0.0)).magnitude() < TOL);
    assert!((t - 0.125).abs() < TOL);
}

#[test]
fn closest_point_clamps_to_segment_endpoint() {
    let p = Polyline::new(vec![point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)])
        .expect("single segment");
    // (5, 0, 0) projects beyond the segment, clamps to (1, 0, 0).
    let (_, q) = p
        .closest_point(&point(5.0, 0.0, 0.0), NORMAL_TOLERANCE)
        .expect("closest");
    assert!((q - point(1.0, 0.0, 0.0)).magnitude() < TOL);
}

#[test]
fn parameters_at_point_returns_t_for_on_curve_point() {
    let sq = unit_square();
    let ts = sq.parameters_at_point(&point(1.0, 0.5, 0.0), NORMAL_TOLERANCE);
    assert_eq!(ts.len(), 1);
    // (1, 0.5, 0) is the mid of segment 1 → t = 0.375.
    assert!((ts[0] - 0.375).abs() < 1e-6);
}

#[test]
fn parameters_at_point_returns_empty_for_off_curve_point() {
    let sq = unit_square();
    let ts = sq.parameters_at_point(&point(0.5, 0.5, 0.0), NORMAL_TOLERANCE);
    assert!(ts.is_empty());
}

// ─── Reversed / transform ──────────────────────────────────────────

#[test]
fn reversed_swaps_endpoints_and_walks_in_reverse() {
    let sq = unit_square();
    let rev = sq.reversed();
    let original_end = sq.evaluate(1.0).unwrap().position;
    let reversed_start = rev.evaluate(0.0).unwrap().position;
    assert!((original_end - reversed_start).magnitude() < TOL);
    let original_start = sq.evaluate(0.0).unwrap().position;
    let reversed_end = rev.evaluate(1.0).unwrap().position;
    assert!((original_start - reversed_end).magnitude() < TOL);
}

#[test]
fn transform_translates_every_vertex() {
    let sq = unit_square();
    let mat = Matrix4::translation(10.0, 0.0, 0.0);
    let moved = sq.transform(&mat);
    let p = moved.evaluate(0.0).unwrap().position;
    assert!((p - point(10.0, 0.0, 0.0)).magnitude() < TOL);
    let p = moved.evaluate(0.25).unwrap().position;
    assert!((p - point(11.0, 0.0, 0.0)).magnitude() < TOL);
}

// ─── Split / subcurve ──────────────────────────────────────────────

#[test]
fn split_at_vertex_partitions_cleanly() {
    let sq = unit_square();
    // Split at t = 0.5 (corner (1, 1, 0)).
    let (a, b) = sq.split(0.5).expect("split");
    // First half ends at the corner.
    let a_end = a.evaluate(1.0).unwrap().position;
    assert!((a_end - point(1.0, 1.0, 0.0)).magnitude() < TOL);
    // Second half starts at the corner.
    let b_start = b.evaluate(0.0).unwrap().position;
    assert!((b_start - point(1.0, 1.0, 0.0)).magnitude() < TOL);
    // Lengths sum to the original perimeter.
    let la = a.arc_length_between(0.0, 1.0, NORMAL_TOLERANCE).unwrap();
    let lb = b.arc_length_between(0.0, 1.0, NORMAL_TOLERANCE).unwrap();
    assert!((la + lb - 4.0).abs() < 1e-6);
}

#[test]
fn split_inside_segment_inserts_split_point() {
    let sq = unit_square();
    // t = 0.125 lands mid-segment-0 at (0.5, 0, 0).
    let (a, b) = sq.split(0.125).expect("split");
    let a_end = a.evaluate(1.0).unwrap().position;
    assert!((a_end - point(0.5, 0.0, 0.0)).magnitude() < TOL);
    let b_start = b.evaluate(0.0).unwrap().position;
    assert!((b_start - point(0.5, 0.0, 0.0)).magnitude() < TOL);
}

#[test]
fn subcurve_spans_endpoints_and_interior_vertices() {
    let sq = unit_square();
    // Subcurve from t = 0.125 (mid seg 0) to t = 0.625 (mid seg 2).
    let sub = sq.subcurve(0.125, 0.625).expect("subcurve");
    let s = sub.evaluate(0.0).unwrap().position;
    assert!((s - point(0.5, 0.0, 0.0)).magnitude() < 1e-6);
    let e = sub.evaluate(1.0).unwrap().position;
    assert!((e - point(0.5, 1.0, 0.0)).magnitude() < 1e-6);
    // Length should be 2.0 (half seg 0 + seg 1 + half seg 2).
    let len = sub.arc_length_between(0.0, 1.0, NORMAL_TOLERANCE).unwrap();
    assert!((len - 2.0).abs() < 1e-6);
}

#[test]
fn subcurve_handles_swapped_args() {
    let sq = unit_square();
    let a = sq.subcurve(0.2, 0.7).unwrap();
    let b = sq.subcurve(0.7, 0.2).unwrap();
    let la = a.arc_length_between(0.0, 1.0, NORMAL_TOLERANCE).unwrap();
    let lb = b.arc_length_between(0.0, 1.0, NORMAL_TOLERANCE).unwrap();
    assert!((la - lb).abs() < 1e-6);
}

// ─── to_nurbs ──────────────────────────────────────────────────────

#[test]
fn to_nurbs_is_degree_one_and_matches_evaluate() {
    let sq = unit_square();
    let nurbs = sq.to_nurbs();
    assert_eq!(nurbs.degree, 1);
    assert_eq!(nurbs.control_points.len(), sq.vertices.len());
    // Spot-check at the corner parameters.
    for t in [0.0_f64, 0.25, 0.5, 0.75, 1.0] {
        let p_poly = sq.evaluate(t).unwrap().position;
        let p_nurbs = nurbs.evaluate(t).unwrap().position;
        assert!(
            (p_poly - p_nurbs).magnitude() < 1e-6,
            "polyline vs NURBS mismatch at t={t}"
        );
    }
}

// ─── Bounding box ─────────────────────────────────────────────────

#[test]
fn bounding_box_spans_all_vertices() {
    let sq = unit_square();
    let (min, max) = sq.bounding_box();
    assert!((min - point(0.0, 0.0, 0.0)).magnitude() < TOL);
    assert!((max - point(1.0, 1.0, 0.0)).magnitude() < TOL);
}

// ─── Intersection ──────────────────────────────────────────────────

#[test]
fn intersect_with_line_through_multiple_segments() {
    let sq = unit_square();
    // Horizontal line y = 0.5 crosses segments 1 (right edge) and
    // 3 (left edge) at (1, 0.5, 0) and (0, 0.5, 0).
    let line = Line::new(point(-5.0, 0.5, 0.0), point(5.0, 0.5, 0.0));
    let hits = sq.intersect_curve(&line, NORMAL_TOLERANCE);
    assert!(
        hits.len() >= 2,
        "expected ≥2 intersections, got {}",
        hits.len()
    );
    // Both hits should land on y = 0.5 with x ∈ {0, 1}.
    for h in &hits {
        assert!((h.point.y - 0.5).abs() < 1e-6);
        assert!(
            (h.point.x - 0.0).abs() < 1e-6 || (h.point.x - 1.0).abs() < 1e-6,
            "unexpected x: {}",
            h.point.x
        );
    }
}

#[test]
fn intersect_plane_returns_segment_crossings() {
    let sq = unit_square();
    // Plane x = 0.5, normal +X. Segments 0 and 2 cross x = 0.5 at their
    // midpoints (t = 0.125 and t = 0.625 in polyline parameter space).
    let plane = Plane::from_point_normal(point(0.5, 0.0, 0.0), Vector3::new(1.0, 0.0, 0.0))
        .expect("plane");
    let ts = sq.intersect_plane(&plane, NORMAL_TOLERANCE);
    assert!(ts.len() >= 2, "expected ≥2 plane crossings, got {}", ts.len());
    let mut sorted = ts.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert!((sorted[0] - 0.125).abs() < 1e-6);
    assert!((sorted[sorted.len() - 1] - 0.625).abs() < 1e-6);
}

// ─── Offset ────────────────────────────────────────────────────────

#[test]
fn offset_open_segment_displaces_by_distance() {
    // Single horizontal segment along +X; offset along normal +Z by 1
    // moves the line by 1 in the +Y direction (Z × X = Y).
    let p = Polyline::new(vec![point(0.0, 0.0, 0.0), point(2.0, 0.0, 0.0)])
        .expect("open polyline");
    let normal = Vector3::new(0.0, 0.0, 1.0);
    let off = p.offset(1.0, &normal).expect("offset");
    let q0 = off.evaluate(0.0).unwrap().position;
    let q1 = off.evaluate(1.0).unwrap().position;
    assert!((q0 - point(0.0, 1.0, 0.0)).magnitude() < 1e-6);
    assert!((q1 - point(2.0, 1.0, 0.0)).magnitude() < 1e-6);
}

#[test]
fn offset_l_shape_uses_miter_at_corner() {
    // L-shape: (0,0) → (1,0) → (1,1). Inward miter offset by 0.1 along
    // normal +Z should land the corner at (1 - 0.1, 0.1, 0) … wait,
    // perpendicular of +X under +Z is +Y; perpendicular of +Y under +Z
    // is -X. So we expect the corner displaced to (1 - 0.1, 0.1, 0) ⇒
    // (0.9, 0.1, 0)? Actually perp of seg 0 (+X) is Z×X = +Y; perp of
    // seg 1 (+Y) is Z×Y = -X. Miter average normalized = (-1, 1)/√2,
    // scaled to keep the offset distance from each segment = 0.1:
    // cos_half = √2/2, so scale = 1/cos_half = √2. Final offset vector:
    // (-1, 1)/√2 · 0.1 · √2 = (-0.1, 0.1). Corner moves to (0.9, 0.1).
    let p = Polyline::new(vec![
        point(0.0, 0.0, 0.0),
        point(1.0, 0.0, 0.0),
        point(1.0, 1.0, 0.0),
    ])
    .expect("L polyline");
    let normal = Vector3::new(0.0, 0.0, 1.0);
    let off = p.offset(0.1, &normal).expect("offset");
    // The middle (corner) vertex is at t = 0.5 for a 2-segment polyline.
    let corner = off.evaluate(0.5).unwrap().position;
    assert!(
        (corner - point(0.9, 0.1, 0.0)).magnitude() < 1e-6,
        "miter corner mismatch: {:?}",
        corner
    );
}

// ─── Continuity ────────────────────────────────────────────────────

#[test]
fn continuity_matches_meeting_endpoints_g0_or_better() {
    let a = Polyline::new(vec![point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)])
        .expect("a");
    let b = Polyline::new(vec![point(1.0, 0.0, 0.0), point(2.0, 0.0, 0.0)])
        .expect("b");
    // Same direction polylines meeting at (1, 0, 0). Tangents agree, so
    // we expect ≥ G0; polylines have zero curvature so the
    // implementation may report G2 (no curvature mismatch).
    let cont = a.check_continuity(&b, true, NORMAL_TOLERANCE);
    assert!(
        matches!(cont, Continuity::G0 | Continuity::G1 | Continuity::G2),
        "unexpected continuity {:?}",
        cont
    );
    assert!(
        !matches!(cont, Continuity::Unknown),
        "continuity should be resolved"
    );
}

#[test]
fn continuity_at_separated_endpoints_returns_g0_at_worst() {
    let a = Polyline::new(vec![point(0.0, 0.0, 0.0), point(1.0, 0.0, 0.0)])
        .expect("a");
    let b = Polyline::new(vec![point(5.0, 0.0, 0.0), point(6.0, 0.0, 0.0)])
        .expect("b");
    // Endpoints far apart → G0 (positional discontinuity).
    let cont = a.check_continuity(&b, true, NORMAL_TOLERANCE);
    assert!(matches!(cont, Continuity::G0));
}

// ─── type_name ─────────────────────────────────────────────────────

#[test]
fn type_name_is_polyline() {
    assert_eq!(unit_square().type_name(), "Polyline");
}
