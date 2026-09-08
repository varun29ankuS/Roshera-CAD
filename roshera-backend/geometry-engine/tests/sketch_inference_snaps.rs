// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::float_cmp)]

//! Task 39 -- an inference snap must propose a constraint about the
//! feature it actually HIT.
//!
//! `Sketch::find_snap_candidates` labels a feature with the entity that
//! OWNS it. For a line segment's endpoint that owner is the LINE, and
//! `inference.rs` turned every discrete snap into
//! `Coincident(draft, snap.entity)`. Two ways that lies:
//!
//! * `Coincident(point, line)` and `Coincident(point, rectangle)` are
//!   shapes the kernel does not define, so the constraint door
//!   (`Sketch::try_add_constraint`, Task 16) refuses them -- a snap the
//!   user can see produces a proposal that cannot be applied;
//! * `Coincident(point, arc)` IS defined, and it means the arc's
//!   CENTRE. An ARC-ENDPOINT snap proposing it is accepted at the door
//!   and, once solved, drags the point to the middle of the arc --
//!   nowhere near the feature the snap reported.
//!
//! These tests pin the contract from both ends: what
//! `find_snap_candidates` NAMES, and what `infer_constraints` proposes
//! -- every proposal must pass the door AND, once solved, leave the
//! point where the snap said it was.

use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, EntityRef, GeometricConstraint,
};
use geometry_engine::sketch2d::inference::{
    infer_constraints, DraftEntity, DraftSlot, InferenceTolerance, ProposedConstraint,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::snap::SnapKind;
use geometry_engine::sketch2d::Point2d;

fn fresh() -> Sketch {
    Sketch::new("inference_snaps".to_string(), SketchAnchor::xy())
}

fn tol() -> InferenceTolerance {
    InferenceTolerance::new(3.0_f64.to_radians(), 0.5, 0.5)
}

/// Pin a point so the solver moves the DRAFT, not the geometry the
/// draft snapped onto.
fn pin(sketch: &Sketch, id: &geometry_engine::sketch2d::point2d::Point2dId) {
    let mut entry = sketch.points().get_mut(id).expect("point present");
    entry.value_mut().fix();
}

/// The single `PointSelf` proposal a draft point's inference produced.
fn point_proposal(sketch: &Sketch, at: Point2d) -> ProposedConstraint {
    let draft = DraftEntity::Point { position: at };
    let out = infer_constraints(sketch, &draft, tol());
    let mut hits = out
        .into_iter()
        .filter(|p| p.draft_slot == DraftSlot::PointSelf);
    let first = hits.next().expect("a snap proposal for the draft point");
    assert!(
        hits.next().is_none(),
        "expected exactly one PointSelf proposal"
    );
    first
}

/// Commit the draft point, walk the proposal through the CHECKED door
/// exactly as the csketch route does, solve, and return where the
/// point ended up.
fn apply_and_solve(
    sketch: &Sketch,
    at: Point2d,
    proposal: &ProposedConstraint,
) -> Result<Point2d, String> {
    let draft = sketch.add_point(at);
    let target = proposal.target.expect("snap proposal names a target");
    let constraint = Constraint::new_geometric(
        proposal.constraint,
        vec![EntityRef::Point(draft), target],
        ConstraintPriority::High,
    );
    sketch
        .try_add_constraint(constraint)
        .map_err(|e| e.to_string())?;
    let report = sketch.solve_constraints().map_err(|e| e.to_string())?;
    assert!(
        report.converged(),
        "expected the solve to converge, got {:?}",
        report.status
    );
    Ok(sketch
        .points()
        .get(&draft)
        .expect("draft point present after solve")
        .value()
        .position)
}

fn assert_near(got: Point2d, want: Point2d, what: &str) {
    assert!(
        (got.x - want.x).abs() < 1e-6 && (got.y - want.y).abs() < 1e-6,
        "{what}: expected ({}, {}), got ({}, {})",
        want.x,
        want.y,
        got.x,
        got.y
    );
}

// ── What the snap NAMES ───────────────────────────────────────────

#[test]
fn line_endpoint_candidate_names_the_endpoints_own_point_entity() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_line(a, b).expect("line");

    let cands = sketch.find_snap_candidates(Point2d::new(10.0, 0.05), 0.5);
    let endpoint = cands
        .iter()
        .find(|c| c.kind == SnapKind::LineEndpoint)
        .expect("a line-endpoint candidate near the end vertex");
    assert_eq!(
        endpoint.entity,
        EntityRef::Point(b),
        "an ENDPOINT hit must name the endpoint's own point entity, not the line that owns it"
    );
}

#[test]
fn arc_endpoint_candidate_names_the_endpoints_own_point_entity() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_arc(a, b, 8.0, true, false).expect("arc");

    let cands = sketch.find_snap_candidates(Point2d::new(0.0, 0.05), 0.5);
    let endpoint = cands
        .iter()
        .find(|c| c.kind == SnapKind::ArcEndpoint)
        .expect("an arc-endpoint candidate near the start vertex");
    assert_eq!(
        endpoint.entity,
        EntityRef::Point(a),
        "an arc ENDPOINT hit must name the endpoint's own point entity, not the arc"
    );
}

#[test]
fn line_endpoint_candidate_keeps_the_line_when_the_endpoint_point_is_gone() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let line = sketch.add_line(a, b).expect("line");
    // `delete_point` does NOT cascade into curves that reference the
    // point, so the segment survives remembering a dead id. Handing
    // that id out would be a dangling entity reference.
    sketch.delete_point(&b).expect("delete endpoint point");

    let cands = sketch.find_snap_candidates(Point2d::new(10.0, 0.05), 0.5);
    let endpoint = cands
        .iter()
        .find(|c| c.kind == SnapKind::LineEndpoint)
        .expect("a line-endpoint candidate near the end vertex");
    assert_eq!(
        endpoint.entity,
        EntityRef::Line(line),
        "a deleted endpoint point must not be named; the owning line stands in"
    );
}

#[test]
fn line_midpoint_candidate_still_names_the_line_that_owns_it() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let line = sketch.add_line(a, b).expect("line");

    let cands = sketch.find_snap_candidates(Point2d::new(5.0, 0.05), 0.5);
    let mid = cands
        .iter()
        .find(|c| c.kind == SnapKind::LineMidpoint)
        .expect("a line-midpoint candidate");
    assert_eq!(
        mid.entity,
        EntityRef::Line(line),
        "a midpoint is not an entity the sketch owns; the line stands for it"
    );
}

// ── What the inference PROPOSES ───────────────────────────────────

#[test]
fn line_endpoint_snap_proposal_lands_the_point_on_the_endpoint() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_line(a, b).expect("line");
    pin(&sketch, &a);
    pin(&sketch, &b);

    let at = Point2d::new(10.0, 0.05);
    let proposal = point_proposal(&sketch, at);
    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    assert_near(landed, Point2d::new(10.0, 0.0), "line endpoint");
}

#[test]
fn arc_endpoint_snap_proposal_lands_the_point_on_the_endpoint() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_arc(a, b, 8.0, true, false).expect("arc");
    pin(&sketch, &a);
    pin(&sketch, &b);

    let at = Point2d::new(0.0, 0.05);
    let proposal = point_proposal(&sketch, at);
    assert_eq!(
        proposal.target,
        Some(EntityRef::Point(a)),
        "an arc endpoint the sketch owns as a point is what the proposal must name"
    );
    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    assert_near(landed, Point2d::new(0.0, 0.0), "arc endpoint");
}

/// An arc built from raw geometry owns no endpoint points, so the
/// arc-endpoint candidate is the best snap in its own right. This is
/// the case where `Coincident(point, arc)` PASSES the door and
/// silently means the arc's CENTRE — the kernel applied a pull to the
/// middle of the arc for a snap the user made on its end.
///
/// The kernel can name no stronger true relation here than "on this
/// arc", so that is what it proposes: weaker than the snap, never
/// wrong, and it leaves the point on the curve it was snapped to
/// instead of dragging it half a radius away.
#[test]
fn legacy_arc_endpoint_snap_proposal_holds_the_point_on_the_arc_not_at_its_centre() {
    let sketch = fresh();
    let arc = sketch
        .add_arc_three_points(
            Point2d::new(-5.0, 0.0),
            Point2d::new(0.0, 5.0),
            Point2d::new(5.0, 0.0),
        )
        .expect("arc");
    let start = {
        let entry = sketch.arcs().get(&arc).expect("arc present");
        entry.value().arc.start_point()
    };

    let at = Point2d::new(start.x, start.y + 0.05);
    let proposal = point_proposal(&sketch, at);
    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    // The arc's own parameters are free in a legacy arc, so read the
    // centre and radius the solve SETTLED on, not the ones it started
    // from.
    let (centre, radius) = {
        let entry = sketch.arcs().get(&arc).expect("arc present after solve");
        (entry.value().arc.center, entry.value().arc.radius)
    };
    let from_centre = ((landed.x - centre.x).powi(2) + (landed.y - centre.y).powi(2)).sqrt();
    assert!(
        from_centre > radius * 0.5,
        "an ENDPOINT snap must not drag the point toward the arc's CENTRE ({}, {}): it landed {} from it, radius {}",
        centre.x,
        centre.y,
        from_centre,
        radius
    );
    assert!(
        (from_centre - radius).abs() < 1e-6,
        "expected the point on the arc (r = {radius}), got r = {from_centre}"
    );
    // Weaker than the snap, but never AWAY from it: holding the point
    // on the curve may not pin it at the endpoint, and it must not
    // push it further off than the cursor already was.
    let before = at.distance_to(&start);
    let after = landed.distance_to(&start);
    assert!(
        after <= before + 1e-9,
        "expected the point no further from the endpoint than it started ({before}), got {after}"
    );
    assert_eq!(
        proposal.constraint,
        GeometricConstraint::PointOnCurve,
        "the arc's endpoint is not an entity; the honest relation is 'on this arc'"
    );
}

#[test]
fn line_midpoint_snap_proposal_lands_the_point_on_the_midpoint() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_line(a, b).expect("line");
    pin(&sketch, &a);
    pin(&sketch, &b);

    let at = Point2d::new(5.0, 0.05);
    let proposal = point_proposal(&sketch, at);
    assert_eq!(
        proposal.constraint,
        GeometricConstraint::Midpoint,
        "a MIDPOINT hit is the Midpoint relation, not a coincidence with the whole line"
    );
    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    assert_near(landed, Point2d::new(5.0, 0.0), "line midpoint");
}

#[test]
fn line_interior_snap_proposal_lands_the_point_on_the_line() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    sketch.add_line(a, b).expect("line");
    pin(&sketch, &a);
    pin(&sketch, &b);

    // 2.5 is neither an endpoint nor the midpoint of [0, 10].
    let at = Point2d::new(2.5, 0.05);
    let proposal = point_proposal(&sketch, at);
    assert_eq!(
        proposal.constraint,
        GeometricConstraint::PointOnCurve,
        "an interior hit is a point ON the curve"
    );
    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    assert!(
        landed.y.abs() < 1e-6,
        "expected the point on the line y = 0, got y = {}",
        landed.y
    );
}

// ── The whole table ───────────────────────────────────────────────

/// Every proposal a DRAFT POINT's inference emits, at every feature of
/// every entity kind it snaps to, must be a shape the constraint door
/// DEFINES. A proposal the door refuses is a snap the user can see and
/// cannot apply.
///
/// Draft LINES and CIRCLES route their snap hits through the same
/// `discrete_snap_relation` table, so this sweep covers the table for
/// them too; what it does not sweep is their direction-driven
/// proposals (Horizontal / Parallel / Tangent / Equal / Concentric),
/// which the inline tests in `inference.rs` cover and which this
/// change did not touch.
#[test]
fn every_snap_proposal_has_a_shape_the_door_defines() {
    let cases: Vec<(&str, Sketch)> = vec![
        ("line", {
            let s = fresh();
            let a = s.add_point(Point2d::new(0.0, 0.0));
            let b = s.add_point(Point2d::new(10.0, 0.0));
            s.add_line(a, b).expect("line");
            s
        }),
        ("shared arc", {
            let s = fresh();
            let a = s.add_point(Point2d::new(0.0, 0.0));
            let b = s.add_point(Point2d::new(10.0, 0.0));
            s.add_arc(a, b, 8.0, true, false).expect("arc");
            s
        }),
        ("legacy arc", {
            let s = fresh();
            s.add_arc_three_points(
                Point2d::new(-5.0, 0.0),
                Point2d::new(0.0, 5.0),
                Point2d::new(5.0, 0.0),
            )
            .expect("arc");
            s
        }),
        ("circle", {
            let s = fresh();
            s.add_circle(Point2d::new(0.0, 0.0), 4.0).expect("circle");
            s
        }),
        ("rectangle", {
            let s = fresh();
            s.add_rectangle(Point2d::new(-3.0, -2.0), Point2d::new(3.0, 2.0))
                .expect("rectangle");
            s
        }),
        ("ellipse", {
            let s = fresh();
            s.add_ellipse(Point2d::new(0.0, 0.0), 6.0, 3.0, 0.4)
                .expect("ellipse");
            s
        }),
    ];

    let mut checked = 0usize;
    for (name, sketch) in &cases {
        // Every feature the snap engine can report, gathered from a
        // wide sweep, then probed one at a time.
        let features = sketch.find_snap_candidates(Point2d::new(0.0, 0.0), 1e6);
        assert!(!features.is_empty(), "{name}: no snap features at all");
        for feature in features {
            let draft = DraftEntity::Point {
                position: feature.point,
            };
            for proposal in infer_constraints(sketch, &draft, tol()) {
                let target = match proposal.target {
                    Some(t) => t,
                    None => continue,
                };
                let probe = Sketch::new("probe".to_string(), SketchAnchor::xy());
                let draft_id = probe.add_point(feature.point);
                let constraint = Constraint::new_geometric(
                    proposal.constraint,
                    vec![EntityRef::Point(draft_id), target],
                    ConstraintPriority::High,
                );
                assert!(
                    constraint.shape_is_defined(),
                    "{name}: a {:?} snap proposed {:?} against a {} -- the door refuses that shape",
                    feature.kind,
                    proposal.constraint,
                    target.kind_name()
                );
                checked += 1;
            }
        }
    }
    assert!(
        checked >= 12,
        "expected the sweep to check a dozen proposals, checked {checked}"
    );
}

/// A rectangle's corner and an ellipse's boundary are positions the
/// kernel defines no constraint for: `Coincident` excludes both kinds,
/// and neither is a `PointOnCurve` carrier. The snap is still reported
/// (the cursor does latch onto them), but no proposal is invented —
/// a proposal the door would refuse is worse than none.
#[test]
fn features_no_constraint_can_place_a_point_at_propose_nothing() {
    let rect = fresh();
    rect.add_rectangle(Point2d::new(-3.0, -2.0), Point2d::new(3.0, 2.0))
        .expect("rectangle");
    let corner = Point2d::new(3.0, 2.0);
    assert!(
        rect.best_snap(corner, 0.5).is_some(),
        "the corner must still snap"
    );
    let out = infer_constraints(
        &rect,
        &DraftEntity::Point {
            position: Point2d::new(3.0, 1.95),
        },
        tol(),
    );
    assert!(
        out.is_empty(),
        "a rectangle corner has no constraint that places a point on it, got {out:?}"
    );

    // An ellipse QUADRANT: a discrete hit with no relation to name.
    let ell = fresh();
    ell.add_ellipse(Point2d::new(0.0, 0.0), 6.0, 3.0, 0.0)
        .expect("ellipse");
    let quadrant = Point2d::new(0.0, 3.05);
    assert!(
        ell.best_snap(quadrant, 0.5)
            .is_some_and(|s| s.kind == SnapKind::EllipseQuadrant),
        "the ellipse quadrant must still snap"
    );
    let out = infer_constraints(&ell, &DraftEntity::Point { position: quadrant }, tol());
    assert!(
        out.is_empty(),
        "an ellipse quadrant has no constraint that places a point on it, got {out:?}"
    );

    // The ellipse BOUNDARY: an on-curve hit, and an ellipse is not a
    // `PointOnCurve` carrier. The semi-axes are genuinely UNEQUAL --
    // when this test was written `Ellipse2d::closest_point` diverged
    // for anything but a circle and the fixture had to use 4x4 to reach
    // the on-curve path at all; Task 49 fixed the solver, so the
    // fixture now exercises the real elliptical case.
    let round = fresh();
    round
        .add_ellipse(Point2d::new(0.0, 0.0), 6.0, 3.0, 0.0)
        .expect("ellipse");
    let k = std::f64::consts::FRAC_1_SQRT_2 * 1.01;
    let on_boundary = Point2d::new(6.0 * k, 3.0 * k);
    assert!(
        round
            .best_snap(on_boundary, 0.5)
            .is_some_and(|s| s.kind == SnapKind::OnEllipse),
        "the ellipse boundary must still snap"
    );
    let out = infer_constraints(
        &round,
        &DraftEntity::Point {
            position: on_boundary,
        },
        tol(),
    );
    assert!(
        out.is_empty(),
        "an ellipse is not a PointOnCurve carrier, got {out:?}"
    );
}

/// A draft LINE whose endpoint lands ON an ellipse must propose
/// nothing about that ellipse.
///
/// Two shapes the door does not define came out of this one gesture.
/// `Tangent` takes 1 line and 1 ROUND curve, and an ellipse is not
/// round -- but `circle_or_arc_center_for` answers for an ellipse, so
/// a line arriving perpendicular to the radius proposed
/// `Tangent(line, ellipse)`. Fall through that and
/// `PointOnCurve(point, ellipse)` came out instead, and an ellipse is
/// not a `PointOnCurve` carrier either.
///
/// The fixture's semi-axes were EQUAL when this was written, because
/// `Ellipse2d::closest_point` diverged for a genuinely elliptical one
/// (16 of 2880 probe positions around a 6x3 ellipse produced an
/// `OnEllipse` candidate within 0.5, and those 16 are only the four
/// axis crossings, where the iteration exits before its first step) and
/// an unequal-axis fixture would
/// have tested nothing. Task 49 replaced that iteration with a
/// bracketed bisection, so the fixture is a real 6x3 ellipse and the
/// on-curve path is reached the way a user reaches it.
#[test]
fn draft_line_endpoint_on_an_ellipse_proposes_nothing_about_the_ellipse() {
    let sketch = fresh();
    sketch
        .add_ellipse(Point2d::new(0.0, 0.0), 6.0, 3.0, 0.0)
        .expect("ellipse");

    // t = pi/4, 1% outside: away from every quadrant (2.74 off the
    // nearest), so the hit is ON-CURVE and not a discrete feature.
    let k = std::f64::consts::FRAC_1_SQRT_2 * 1.01;
    let start = Point2d::new(6.0 * k, 3.0 * k);
    assert!(
        sketch
            .best_snap(start, 0.5)
            .is_some_and(|s| s.kind == SnapKind::OnEllipse),
        "the fixture must produce an ON-ELLIPSE snap, not a quadrant"
    );
    // Perpendicular to the centre -> start radius: the direction that
    // used to read as tangency.
    let end = Point2d::new(start.x - start.y, start.y + start.x);

    let out = infer_constraints(&sketch, &DraftEntity::Line { start, end }, tol());
    assert!(
        out.iter()
            .all(|p| !matches!(p.target, Some(EntityRef::Ellipse(_)))),
        "no constraint the kernel defines pairs a line with an ellipse, got {out:?}"
    );
}

// ── The weakened arm: a position ON a round curve ─────────────────
//
// `LineEndpoint | ArcEndpoint | ArcMidpoint | CircleQuadrant` all
// resolve to `PointOnCurve` against the owning round curve. This is
// the arm the whole task turns on, and it is the one arm
// `every_snap_proposal_has_a_shape_the_door_defines` CANNOT guard:
// `Coincident(point, circle)` and `Coincident(point, arc)` are shapes
// the door DEFINES, so the sweep passes on them. The sweep checks
// SHAPE; these two tests check MEANING -- they solve the proposal and
// look at where the point actually went. A quadrant or an arc midpoint
// answered with `Coincident` is dragged a full radius to the curve's
// CENTRE, which no shape check can see.

#[test]
fn circle_quadrant_snap_proposal_holds_the_point_on_the_circle_not_at_its_centre() {
    let sketch = fresh();
    let circle = sketch
        .add_circle(Point2d::new(0.0, 0.0), 5.0)
        .expect("circle");

    // The +X quadrant at (5, 0). `CircleQuadrant` is priority 1 and so
    // outranks the priority-2 `OnCircle` foot at the same distance;
    // the centre is 5 away, outside the 0.5 snap radius.
    let at = Point2d::new(5.0, 0.05);
    let proposal = point_proposal(&sketch, at);
    assert_eq!(proposal.target, Some(EntityRef::Circle(circle)));

    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    let (centre, radius) = {
        let entry = sketch.circles().get(&circle).expect("circle after solve");
        (entry.value().circle.center, entry.value().circle.radius)
    };
    let from_centre = ((landed.x - centre.x).powi(2) + (landed.y - centre.y).powi(2)).sqrt();
    assert!(
        from_centre > radius * 0.5,
        "a QUADRANT snap must not drag the point toward the centre ({}, {}): it landed {} from it, radius {}",
        centre.x,
        centre.y,
        from_centre,
        radius
    );
    assert!(
        (from_centre - radius).abs() < 1e-6,
        "expected the point on the circle (r = {radius}), got r = {from_centre}"
    );
    let quadrant = Point2d::new(5.0, 0.0);
    assert!(
        landed.distance_to(&quadrant) <= at.distance_to(&quadrant) + 1e-9,
        "expected the point no further from the quadrant than it started"
    );
    // The LABEL, checked last: the solved position above is the claim,
    // this is only the name for it.
    assert_eq!(
        proposal.constraint,
        GeometricConstraint::PointOnCurve,
        "a QUADRANT is a position on the circle; Coincident against a circle is its CENTRE"
    );
}

#[test]
fn arc_midpoint_snap_proposal_holds_the_point_on_the_arc_not_at_its_centre() {
    let sketch = fresh();
    // A three-point arc: centre (0, 0), radius 5, parametric midpoint
    // at (0, 5). Both endpoints are 7.07 away and the centre 5.05 --
    // all outside the 0.5 snap radius, so the midpoint is the hit.
    let arc = sketch
        .add_arc_three_points(
            Point2d::new(-5.0, 0.0),
            Point2d::new(0.0, 5.0),
            Point2d::new(5.0, 0.0),
        )
        .expect("arc");

    let at = Point2d::new(0.0, 5.05);
    let proposal = point_proposal(&sketch, at);
    assert_eq!(proposal.target, Some(EntityRef::Arc(arc)));

    let landed =
        apply_and_solve(&sketch, at, &proposal).expect("the door must accept the proposal");
    let (centre, radius) = {
        let entry = sketch.arcs().get(&arc).expect("arc after solve");
        (entry.value().arc.center, entry.value().arc.radius)
    };
    let from_centre = ((landed.x - centre.x).powi(2) + (landed.y - centre.y).powi(2)).sqrt();
    assert!(
        from_centre > radius * 0.5,
        "an arc MIDPOINT snap must not drag the point toward the centre ({}, {}): it landed {} from it, radius {}",
        centre.x,
        centre.y,
        from_centre,
        radius
    );
    assert!(
        (from_centre - radius).abs() < 1e-6,
        "expected the point on the arc (r = {radius}), got r = {from_centre}"
    );
    let midpoint = Point2d::new(0.0, 5.0);
    assert!(
        landed.distance_to(&midpoint) <= at.distance_to(&midpoint) + 1e-9,
        "expected the point no further from the arc midpoint than it started"
    );
    assert_eq!(
        proposal.constraint,
        GeometricConstraint::PointOnCurve,
        "an arc MIDPOINT is a position on the arc; Coincident against an arc is its CENTRE"
    );
}
