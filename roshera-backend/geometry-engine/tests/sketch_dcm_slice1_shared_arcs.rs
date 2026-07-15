//! SKETCH-DCM campaign #45, Slice 1 — arcs + circles in the
//! shared-variable entity model.
//!
//! RED contract (written before the solver implementation): a
//! "closed" line-arc slot profile — 4 points, 2 lines, 2 arcs, all
//! joined at the 4 shared points — must
//!
//! 1. report EXACTLY the hand-counted DOF total (no phantom arc DOFs:
//!    an endpoint-derived arc owns ONE private parameter, the center's
//!    signed offset along the chord's perpendicular bisector — its
//!    endpoint coordinates live in the shared points and must not be
//!    double-counted);
//! 2. stay welded at the seams after a dimensioned solve — the arc's
//!    endpoints ARE the shared points (exactly — the same coordinates
//!    the adjacent line reads, not within-epsilon-after-luck), and the
//!    arc's derived geometry passes through them;
//! 3. follow a dragged shared endpoint with BOTH incident entities
//!    (line and arc).
//!
//! Plus the circle half of the slice: a circle whose center is a
//! shared point is concentric-by-construction (1 private DOF — the
//! radius) and follows its dragged center point.
//!
//! Before Slice 1 the arc carried a private 5-parameter copy
//! (cx, cy, r, a0, a1) with no coupling to point entities: the slot
//! over-reported DOF by 4 per arc and solved slots came apart at the
//! line/arc seams. These tests pin that defect.

#![allow(clippy::float_cmp)]
// Reason for `#![allow(clippy::expect_used)]` / `unwrap_used` —
// test-only file: failing loudly at the fixture site is the desired
// failure mode; the workspace deny lints target production code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef,
};
use geometry_engine::sketch2d::line2d::LineGeometry;
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_solver::DragTarget;
use geometry_engine::sketch2d::Point2d;
use std::f64::consts::PI;

fn fresh() -> Sketch {
    Sketch::new("dcm_slice1".to_string(), SketchAnchor::xy())
}

fn dist(a: Point2d, b: Point2d) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

/// Slot profile:
/// ```text
///        D(0,10) ───────── C(20,10)
///      (          line dc           )
///  left arc                     right arc   (radius 7 at creation)
///      (          line ab           )
///        A(0,0)  ───────── B(20,0)
/// ```
/// Lines and arcs all reference the four shared `Point2dId`s.
struct Slot {
    sketch: Sketch,
    a: geometry_engine::sketch2d::Point2dId,
    b: geometry_engine::sketch2d::Point2dId,
    c: geometry_engine::sketch2d::Point2dId,
    d: geometry_engine::sketch2d::Point2dId,
    ab: geometry_engine::sketch2d::Line2dId,
    dc: geometry_engine::sketch2d::Line2dId,
    right: geometry_engine::sketch2d::Arc2dId,
    left: geometry_engine::sketch2d::Arc2dId,
}

fn build_slot() -> Slot {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(20.0, 0.0));
    let c = sketch.add_point(Point2d::new(20.0, 10.0));
    let d = sketch.add_point(Point2d::new(0.0, 10.0));

    let ab = sketch.add_line(a, b).expect("line ab");
    let dc = sketch.add_line(d, c).expect("line dc");

    // End caps: chord length 10, creation radius 7 (minor arcs).
    let right = sketch.add_arc(b, c, 7.0, true, false).expect("right arc");
    let left = sketch.add_arc(d, a, 7.0, true, false).expect("left arc");

    Slot {
        sketch,
        a,
        b,
        c,
        d,
        ab,
        dc,
        right,
        left,
    }
}

/// Fully dimension the slot: pin all four points by absolute
/// coordinates (8 × 1 DOF) and dimension both cap radii to 6 mm
/// (2 × 1 DOF).
fn dimension_slot(slot: &Slot) {
    let coords: [(geometry_engine::sketch2d::Point2dId, f64, f64); 4] = [
        (slot.a, 0.0, 0.0),
        (slot.b, 20.0, 0.0),
        (slot.c, 20.0, 10.0),
        (slot.d, 0.0, 10.0),
    ];
    for (p, x, y) in coords {
        slot.sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(x),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        ));
        slot.sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(y),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        ));
    }
    for arc in [slot.right, slot.left] {
        slot.sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Radius(6.0),
            vec![EntityRef::Arc(arc)],
            ConstraintPriority::Required,
        ));
    }
}

fn segment_of(sketch: &Sketch, id: &geometry_engine::sketch2d::Line2dId) -> (Point2d, Point2d) {
    let entry = sketch.lines().get(id).expect("line present");
    match entry.value().geometry {
        LineGeometry::Segment(s) => (s.start, s.end),
        ref other => panic!("expected segment, got {:?}", other),
    }
}

// ── (a) Exact DOF hand-count ───────────────────────────────────────

#[test]
fn slot_dof_matches_hand_count_exactly() {
    let slot = build_slot();
    dimension_slot(&slot);

    // Hand count (shared-variable model):
    //   4 points               × 2 DOF (x, y)                    =  8
    //   2 endpoint-derived lines × 0 DOF (geometry IS the points) =  0
    //   2 endpoint-derived arcs  × 1 DOF (center offset along the
    //     chord's perpendicular bisector; endpoints live in the
    //     shared points, radius/center/angles derive)             =  2
    //   TOTAL free                                                = 10
    //
    //   removed: 8 × XCoordinate/YCoordinate (1 each)             =  8
    //          + 2 × Radius (1 each)                              =  2
    //                                                             = 10
    //   ⇒ 10 − 10 = 0 remaining: FullyConstrained.
    let report = slot.sketch.analyze_dofs();
    assert_eq!(
        report.total_free_dofs, 10,
        "phantom arc DOFs: expected the hand-counted 10 free DOFs \
         (endpoint-derived arcs contribute 1, not 5), got {} — status {:?}",
        report.total_free_dofs, report.status
    );
    assert_eq!(report.constraint_dofs_removed, 10);
    assert!(
        report.is_fully_constrained(),
        "expected FullyConstrained, got {:?}",
        report.status
    );
}

// ── (b) Seam exactness after a dimensioned solve ───────────────────

#[test]
fn solved_slot_stays_welded_at_line_arc_seams() {
    let slot = build_slot();
    dimension_slot(&slot);

    let report = slot.sketch.solve_constraints().expect("solve");

    let a_pos = slot.sketch.get_point(&slot.a).expect("a");
    let b_pos = slot.sketch.get_point(&slot.b).expect("b");
    let c_pos = slot.sketch.get_point(&slot.c).expect("c");
    let d_pos = slot.sketch.get_point(&slot.d).expect("d");

    // Lines read their geometry from the shared points — exactly.
    let (ab_start, ab_end) = segment_of(&slot.sketch, &slot.ab);
    assert_eq!(ab_start, a_pos);
    assert_eq!(ab_end, b_pos);
    let (dc_start, dc_end) = segment_of(&slot.sketch, &slot.dc);
    assert_eq!(dc_start, d_pos);
    assert_eq!(dc_end, c_pos);

    // Arc endpoints ARE the shared points — the same coordinates the
    // lines see, bit for bit, not within-epsilon-after-luck.
    let (r_start, r_end) = slot
        .sketch
        .arc_endpoint_positions(&slot.right)
        .expect("right arc endpoints");
    assert_eq!(r_start, ab_end, "right arc start must BE line ab's end");
    assert_eq!(r_end, dc_end, "right arc end must BE line dc's end");
    let (l_start, l_end) = slot
        .sketch
        .arc_endpoint_positions(&slot.left)
        .expect("left arc endpoints");
    assert_eq!(l_start, dc_start, "left arc start must BE line dc's start");
    assert_eq!(l_end, ab_start, "left arc end must BE line ab's start");

    // And the arcs' DERIVED geometry (center, radius, angles) passes
    // through those points: the stored Arc2d cannot disagree with its
    // endpoints after a solve.
    for (arc_id, s_pos, e_pos, tag) in [
        (slot.right, b_pos, c_pos, "right"),
        (slot.left, d_pos, a_pos, "left"),
    ] {
        let entry = slot.sketch.arcs().get(&arc_id).expect("arc present");
        let arc = entry.value().arc;
        assert!(
            dist(arc.start_point(), s_pos) < 1e-9,
            "{tag} arc geometry start drifted off its shared point: \
             {:?} vs {:?} (gap {})",
            arc.start_point(),
            s_pos,
            dist(arc.start_point(), s_pos)
        );
        assert!(
            dist(arc.end_point(), e_pos) < 1e-9,
            "{tag} arc geometry end drifted off its shared point: \
             {:?} vs {:?} (gap {})",
            arc.end_point(),
            e_pos,
            dist(arc.end_point(), e_pos)
        );
        assert!(
            (arc.radius - 6.0).abs() < 1e-6,
            "{tag} arc radius did not reach its 6.0 dimension: {}",
            arc.radius
        );
    }

    // The dimensioned slot converges: the constraint set removes
    // exactly the free DOFs, so the DOF verdict is clean and Newton
    // drives the residual under tolerance.
    assert!(
        report.converged(),
        "expected convergence on the fully-dimensioned slot, got {:?}",
        report.status
    );
}

// ── (c) Dragging a shared endpoint moves BOTH incident entities ────

#[test]
fn dragging_shared_endpoint_moves_line_and_arc_together() {
    let slot = build_slot();
    let target = Point2d::new(25.0, -2.0);

    slot.sketch
        .solve_drag(EntityRef::Point(slot.b), DragTarget::Point(target))
        .expect("drag");

    let b_pos = slot.sketch.get_point(&slot.b).expect("b");
    assert!(
        dist(b_pos, target) < 1e-3,
        "drag did not pull the free point onto the cursor: {:?}",
        b_pos
    );

    // Line ab follows (shared-variable model for lines, already live).
    let (_, ab_end) = segment_of(&slot.sketch, &slot.ab);
    assert_eq!(ab_end, b_pos, "line ab must track its dragged endpoint");

    // The arc's endpoint IS the dragged point…
    let (r_start, _) = slot
        .sketch
        .arc_endpoint_positions(&slot.right)
        .expect("right arc endpoints");
    assert_eq!(r_start, b_pos, "right arc start must BE the dragged point");

    // …and the arc's derived geometry follows it too.
    let entry = slot.sketch.arcs().get(&slot.right).expect("arc present");
    let arc = entry.value().arc;
    assert!(
        dist(arc.start_point(), b_pos) < 1e-6,
        "right arc geometry did not follow the dragged shared endpoint: \
         arc start {:?} vs point {:?} (gap {})",
        arc.start_point(),
        b_pos,
        dist(arc.start_point(), b_pos)
    );
}

// ── Circles: concentric-by-construction shared center ──────────────

#[test]
fn shared_center_circles_are_concentric_by_construction() {
    let sketch = fresh();
    let p = sketch.add_point(Point2d::new(5.0, 5.0));
    let c1 = sketch.add_circle_centered(p, 3.0).expect("inner circle");
    let c2 = sketch.add_circle_centered(p, 8.0).expect("outer circle");

    // Hand count: point 2 + circle 1 (radius) + circle 1 (radius) = 4.
    // The center coordinates live ONCE, in the shared point.
    let report = sketch.analyze_dofs();
    assert_eq!(
        report.total_free_dofs, 4,
        "shared-center circles must contribute 1 DOF each (radius), \
         got total {} — status {:?}",
        report.total_free_dofs, report.status
    );

    // Drag the shared center: BOTH circles follow, exactly.
    let target = Point2d::new(9.0, 2.0);
    sketch
        .solve_drag(EntityRef::Point(p), DragTarget::Point(target))
        .expect("drag");
    let p_pos = sketch.get_point(&p).expect("p");
    assert!(dist(p_pos, target) < 1e-3, "center did not track cursor");

    for (id, r, tag) in [(c1, 3.0, "inner"), (c2, 8.0, "outer")] {
        let entry = sketch.circles().get(&id).expect("circle present");
        let circle = entry.value().circle;
        assert_eq!(
            circle.center, p_pos,
            "{tag} circle center must BE the shared point after drag"
        );
        assert!(
            (circle.radius - r).abs() < 1e-9,
            "{tag} circle radius must be untouched by a center drag: {}",
            circle.radius
        );
    }

    // Concentricity is structural, not lucky: both centers are the
    // same coordinates because they are the same point.
    let center1 = sketch.circle_center_position(&c1).expect("c1 center");
    let center2 = sketch.circle_center_position(&c2).expect("c2 center");
    assert_eq!(center1, center2);
    assert_eq!(center1, p_pos);
}

// ── Arcs: shared center (concentric-by-construction) ───────────────

#[test]
fn shared_center_arc_follows_dragged_center_point() {
    let sketch = fresh();
    let cp = sketch.add_point(Point2d::new(0.0, 0.0));
    let arc_id = sketch
        .add_arc_centered(cp, 5.0, 0.0, PI / 2.0)
        .expect("centered arc");

    // Hand count: point 2 + arc 3 (radius, start_angle, end_angle) = 5.
    let report = sketch.analyze_dofs();
    assert_eq!(
        report.total_free_dofs, 5,
        "center-shared arc must contribute 3 DOF (r, a0, a1), got \
         total {} — status {:?}",
        report.total_free_dofs, report.status
    );

    let target = Point2d::new(3.0, 4.0);
    sketch
        .solve_drag(EntityRef::Point(cp), DragTarget::Point(target))
        .expect("drag");
    let cp_pos = sketch.get_point(&cp).expect("cp");
    assert!(dist(cp_pos, target) < 1e-3, "center did not track cursor");

    let entry = sketch.arcs().get(&arc_id).expect("arc present");
    let arc = entry.value().arc;
    assert_eq!(
        arc.center, cp_pos,
        "arc center must BE the shared point after drag"
    );
    assert!(
        (arc.radius - 5.0).abs() < 1e-9,
        "radius must be untouched by a center drag: {}",
        arc.radius
    );
    assert!(
        arc.start_angle.abs() < 1e-9 && (arc.end_angle - PI / 2.0).abs() < 1e-9,
        "angles must be untouched by a center drag: ({}, {})",
        arc.start_angle,
        arc.end_angle
    );
}

// ── Backward compatibility: legacy arcs/circles are untouched ──────

#[test]
fn legacy_arcs_and_circles_keep_private_parameter_counts() {
    let sketch = fresh();
    // Legacy creation paths: raw geometry, no shared refs.
    let arc_id = sketch
        .add_arc_center_angles(Point2d::new(0.0, 0.0), 5.0, 0.0, PI / 2.0)
        .expect("legacy arc");
    let circle_id = sketch
        .add_circle(Point2d::new(10.0, 0.0), 2.0)
        .expect("legacy circle");

    assert!(sketch.arcs().get(&arc_id).expect("arc").endpoints.is_none());
    assert!(sketch
        .arcs()
        .get(&arc_id)
        .expect("arc")
        .center_point
        .is_none());
    assert!(sketch
        .circles()
        .get(&circle_id)
        .expect("circle")
        .center_point
        .is_none());

    // Legacy DOF accounting is byte-identical: arc 5, circle 3.
    let report = sketch.analyze_dofs();
    assert_eq!(report.total_free_dofs, 8);
}
