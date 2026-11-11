// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! SK-C3: a sketch profile that crosses or touches itself must never certify
//! `self_intersection_free`, whatever its curves are made of.
//!
//! The check used to cover polylines only: a bow-tie drawn as four separate
//! lines, a spline looping over itself, an ellipse cut by a line — all read
//! self-intersection free. The controls are the joins that look like contacts
//! to a careless check and are not: tangent arcs in a slot, a fillet, a spline
//! flowing out of a line, a closed spline, a circle split into two arcs.

use std::f64::consts::{FRAC_PI_2, PI};

use geometry_engine::sketch2d::{Point2d, Sketch, SketchAnchor};

fn sketch() -> Sketch {
    Sketch::new("crossings".to_string(), SketchAnchor::xy())
}

fn line(s: &Sketch, a: (f64, f64), b: (f64, f64)) {
    let pa = s.add_point(Point2d::new(a.0, a.1));
    let pb = s.add_point(Point2d::new(b.0, b.1));
    s.add_line(pa, pb).expect("line");
}

fn cubic(s: &Sketch, cps: [(f64, f64); 4]) {
    let cps = cps.iter().map(|&(x, y)| Point2d::new(x, y)).collect();
    s.add_bspline(3, cps, vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0])
        .expect("cubic");
}

fn free(s: &Sketch) -> bool {
    let cert = s.certify();
    cert.self_intersection_free
}

// ── Crossings that must be caught ────────────────────────────────────────

#[test]
fn a_bow_tie_of_four_lines_is_not_self_intersection_free() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 10.0));
    line(&s, (10.0, 10.0), (10.0, 0.0));
    line(&s, (10.0, 0.0), (0.0, 10.0));
    line(&s, (0.0, 10.0), (0.0, 0.0));
    let cert = s.certify();
    assert!(!cert.self_intersection_free, "{:?}", cert.issues);
    assert!(!cert.is_sound());
}

#[test]
fn a_line_through_a_circle_is_caught() {
    let s = sketch();
    s.add_circle(Point2d::new(0.0, 0.0), 5.0).expect("circle");
    line(&s, (-10.0, 0.0), (10.0, 0.0));
    assert!(!free(&s));
}

#[test]
fn a_line_through_an_ellipse_is_caught() {
    let s = sketch();
    s.add_ellipse(Point2d::new(0.0, 0.0), 8.0, 3.0, 0.3)
        .expect("ellipse");
    line(&s, (-10.0, 1.0), (10.0, 1.0));
    assert!(!free(&s));
}

#[test]
fn two_crossing_ellipses_are_caught() {
    let s = sketch();
    s.add_ellipse(Point2d::new(0.0, 0.0), 8.0, 3.0, 0.0)
        .expect("ellipse");
    s.add_ellipse(Point2d::new(0.0, 0.0), 8.0, 3.0, FRAC_PI_2)
        .expect("ellipse");
    assert!(!free(&s));
}

#[test]
fn a_spline_crossing_a_line_is_caught() {
    let s = sketch();
    cubic(&s, [(0.0, -5.0), (6.0, -2.0), (-6.0, 2.0), (0.0, 5.0)]);
    line(&s, (-10.0, 0.0), (10.0, 0.0));
    assert!(!free(&s));
}

#[test]
fn a_spline_that_loops_over_itself_is_caught() {
    let s = sketch();
    // Crosses itself near (5, 4.28).
    cubic(&s, [(0.0, 0.0), (15.0, 10.0), (-5.0, 10.0), (10.0, 0.0)]);
    assert!(!free(&s));
}

#[test]
fn two_crossing_splines_are_caught() {
    let s = sketch();
    cubic(&s, [(0.0, -5.0), (6.0, -2.0), (-6.0, 2.0), (0.0, 5.0)]);
    cubic(&s, [(-5.0, 0.0), (-2.0, 6.0), (2.0, -6.0), (5.0, 0.0)]);
    assert!(!free(&s));
}

#[test]
fn a_spline_crossing_an_ellipse_is_caught() {
    let s = sketch();
    s.add_ellipse(Point2d::new(0.0, 0.0), 4.0, 2.0, 0.0)
        .expect("ellipse");
    cubic(&s, [(-8.0, 0.0), (-3.0, 3.0), (3.0, -3.0), (8.0, 0.0)]);
    assert!(!free(&s));
}

#[test]
fn a_line_ending_on_another_lines_interior_is_caught() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    line(&s, (5.0, 0.0), (5.0, 5.0));
    assert!(!free(&s), "a T-junction is a contact, not a join");
}

#[test]
fn collinear_overlapping_lines_are_caught() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    line(&s, (4.0, 0.0), (14.0, 0.0));
    assert!(!free(&s));
}

#[test]
fn overlapping_arcs_of_one_circle_are_caught() {
    let s = sketch();
    s.add_arc_center_angles(Point2d::new(0.0, 0.0), 5.0, 0.0, PI)
        .expect("arc");
    s.add_arc_center_angles(Point2d::new(0.0, 0.0), 5.0, FRAC_PI_2, 1.5 * PI)
        .expect("arc");
    assert!(!free(&s));
}

#[test]
fn a_line_tangent_to_a_circle_away_from_any_join_is_a_contact() {
    let s = sketch();
    s.add_circle(Point2d::new(0.0, 0.0), 5.0).expect("circle");
    line(&s, (-10.0, 5.0), (10.0, 5.0));
    assert!(!free(&s));
}

#[test]
fn a_line_crossing_a_rectangle_and_a_polyline_is_caught() {
    let s = sketch();
    s.add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(10.0, 6.0))
        .expect("rect");
    s.add_polyline(
        vec![
            Point2d::new(-2.0, 3.0),
            Point2d::new(5.0, 3.0),
            Point2d::new(5.0, 8.0),
        ],
        false,
    )
    .expect("polyline");
    assert!(!free(&s));
}

// ── Joins and separations that must stay clean ───────────────────────────

#[test]
fn a_closed_rectangle_of_lines_is_free() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    line(&s, (10.0, 0.0), (10.0, 6.0));
    line(&s, (10.0, 6.0), (0.0, 6.0));
    line(&s, (0.0, 6.0), (0.0, 0.0));
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_slot_of_lines_and_tangent_arcs_is_free() {
    let s = sketch();
    line(&s, (0.0, -2.0), (10.0, -2.0));
    line(&s, (10.0, 2.0), (0.0, 2.0));
    s.add_arc_center_angles(Point2d::new(10.0, 0.0), 2.0, -FRAC_PI_2, FRAC_PI_2)
        .expect("arc");
    s.add_arc_center_angles(Point2d::new(0.0, 0.0), 2.0, FRAC_PI_2, 1.5 * PI)
        .expect("arc");
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_rectangle_with_a_filleted_corner_is_free() {
    let s = sketch();
    line(&s, (0.0, 0.0), (8.0, 0.0));
    s.add_arc_center_angles(Point2d::new(8.0, 2.0), 2.0, -FRAC_PI_2, 0.0)
        .expect("fillet");
    line(&s, (10.0, 2.0), (10.0, 6.0));
    line(&s, (10.0, 6.0), (0.0, 6.0));
    line(&s, (0.0, 6.0), (0.0, 0.0));
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_spline_flowing_tangentially_out_of_a_line_is_free() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    cubic(&s, [(10.0, 0.0), (13.0, 0.0), (16.0, 4.0), (12.0, 8.0)]);
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn two_splines_joined_tangentially_are_free() {
    let s = sketch();
    cubic(&s, [(0.0, 0.0), (3.0, 3.0), (6.0, 3.0), (9.0, 0.0)]);
    cubic(&s, [(9.0, 0.0), (12.0, -3.0), (15.0, -3.0), (18.0, 0.0)]);
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_closed_spline_is_free() {
    let s = sketch();
    let cps = [
        (0.0, 0.0),
        (4.0, -2.0),
        (8.0, 0.0),
        (8.0, 6.0),
        (4.0, 8.0),
        (0.0, 6.0),
        (0.0, 0.0),
    ]
    .iter()
    .map(|&(x, y)| Point2d::new(x, y))
    .collect();
    s.add_bspline(
        3,
        cps,
        vec![0.0, 0.0, 0.0, 0.0, 0.25, 0.5, 0.75, 1.0, 1.0, 1.0, 1.0],
    )
    .expect("closed spline");
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_circle_and_an_ellipse_inside_a_rectangle_are_free() {
    let s = sketch();
    s.add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(30.0, 12.0))
        .expect("rect");
    s.add_circle(Point2d::new(6.0, 6.0), 3.0).expect("circle");
    s.add_ellipse(Point2d::new(20.0, 6.0), 6.0, 3.0, 0.2)
        .expect("ellipse");
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_circle_drawn_as_two_arcs_is_free() {
    let s = sketch();
    s.add_arc_center_angles(Point2d::new(0.0, 0.0), 5.0, 0.0, PI)
        .expect("arc");
    s.add_arc_center_angles(Point2d::new(0.0, 0.0), 5.0, PI, 2.0 * PI)
        .expect("arc");
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn construction_geometry_never_counts() {
    let s = sketch();
    s.add_circle(Point2d::new(0.0, 0.0), 5.0).expect("circle");
    let pa = s.add_point(Point2d::new(-10.0, 0.0));
    let pb = s.add_point(Point2d::new(10.0, 0.0));
    let id = s.add_line(pa, pb).expect("line");
    s.set_construction(&geometry_engine::sketch2d::EntityRef::Line(id), true)
        .expect("construction");
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

// ── Joins at both ends, higher-order joins, rational splines ─────────────

#[test]
fn a_spline_closing_a_d_shape_on_one_line_is_free() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    cubic(&s, [(10.0, 0.0), (10.0, 8.0), (0.0, 8.0), (0.0, 0.0)]);
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_spline_ending_on_a_lines_interior_is_caught() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    cubic(&s, [(5.0, 0.0), (5.0, 3.0), (8.0, 3.0), (8.0, 6.0)]);
    let cert = s.certify();
    assert!(!cert.self_intersection_free);
    assert!(
        cert.profile_contacts
            .iter()
            .any(|c| c.kind
                == geometry_engine::sketch2d::profile_contacts::ContactKind::EndOnInterior),
        "{:?}",
        cert.profile_contacts
    );
}

#[test]
fn a_nurbs_quarter_circle_tangent_to_a_line_is_free() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    s.add_nurbs(
        2,
        vec![
            Point2d::new(10.0, 0.0),
            Point2d::new(12.0, 0.0),
            Point2d::new(12.0, 2.0),
        ],
        vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0],
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
    )
    .expect("nurbs");
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_curvature_continuous_join_is_free() {
    // P0, P1, P2 collinear with the line: the join is G2 (a triple root).
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 0.0));
    cubic(&s, [(10.0, 0.0), (12.0, 0.0), (14.0, 0.0), (16.0, 3.0)]);
    let cert = s.certify();
    assert!(cert.self_intersection_free, "{:?}", cert.issues);
}

#[test]
fn a_line_ending_on_an_ellipse_is_caught() {
    let s = sketch();
    s.add_ellipse(Point2d::new(0.0, 0.0), 6.0, 3.0, 0.0)
        .expect("ellipse");
    line(&s, (6.0, 0.0), (12.0, 0.0));
    assert!(!free(&s));
}

#[test]
fn the_bow_tie_contact_names_the_two_crossing_lines() {
    let s = sketch();
    line(&s, (0.0, 0.0), (10.0, 10.0));
    line(&s, (10.0, 10.0), (10.0, 0.0));
    line(&s, (10.0, 0.0), (0.0, 10.0));
    line(&s, (0.0, 10.0), (0.0, 0.0));
    let cert = s.certify();
    assert_eq!(
        cert.profile_contacts.len(),
        1,
        "{:?}",
        cert.profile_contacts
    );
    let c = &cert.profile_contacts[0];
    assert_eq!(
        c.kind,
        geometry_engine::sketch2d::profile_contacts::ContactKind::Crossing
    );
    assert!((c.point[0] - 5.0).abs() < 1e-9 && (c.point[1] - 5.0).abs() < 1e-9);
    assert!(
        c.first.contains("Line") || c.first.to_lowercase().contains("line"),
        "{}",
        c.first
    );
    assert!(cert.unchecked_pairs.is_empty());
}
