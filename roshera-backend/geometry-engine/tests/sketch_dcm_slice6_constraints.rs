// Reason: integration-test crate -- panicking (unwrap/expect/assert/index) is
// the test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
#![allow(clippy::indexing_slicing)]

//! SKETCH-DCM #45 Slice 6 — the honest-refuse constraint burndown
//! (spec §2.2 + §3.5 Slice 6).
//!
//! Five of the eight honest-refuse variants gain REAL residual
//! equations this slice:
//!
//!   - `GeometricConstraint::Offset`        — offset-pair correspondence
//!   - `DimensionalConstraint::OffsetDistance(d)` — offset-gap magnitude
//!   - `GeometricConstraint::MultiTangent`  — one line tangent to N curves
//!   - `DimensionalConstraint::MinDistance(d)` — one-sided inequality
//!   - `DimensionalConstraint::MaxDistance(d)` — one-sided inequality
//!
//! The remaining three (`CurvatureExtremum`, `ContactConstraint`,
//! `MomentOfInertia`) STAY refused — pinned here so the refuse
//! contract cannot silently erode. Inequalities remove ZERO DOF by
//! design (the D-Cubed convention: a satisfied inequality is inactive
//! and consumes no freedom); their honesty comes from the one-sided
//! residual, which is non-zero exactly when the bound is violated.
use geometry_engine::sketch2d::{
    Constraint, ConstraintPriority, ConstraintRole, DimensionalConstraint, DofStatus, EntityRef,
    GeometricConstraint, Point2d, Sketch, SketchAnchor,
};

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

fn fix_point(sketch: &Sketch, id: &geometry_engine::sketch2d::Point2dId) {
    sketch
        .points()
        .get_mut(id)
        .expect("point present")
        .value_mut()
        .fix();
}

fn required_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::Required)
}

fn high_geo(gc: GeometricConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_geometric(gc, entities, ConstraintPriority::High)
}

// ── Min/MaxDistance: solver inequalities (active-set style) ─────────

#[test]
fn red_min_distance_pushes_a_too_close_point_out_to_the_bound() {
    let s = fresh("slice6_min_distance_active");
    let anchor = s.add_point(Point2d::new(0.0, 0.0));
    fix_point(&s, &anchor);
    let p = s.add_point(Point2d::new(3.0, 0.0));
    // Keep the solve deterministic: pin p to the X axis.
    s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(p)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::MinDistance(5.0),
        vec![EntityRef::Point(anchor), EntityRef::Point(p)],
    ));

    let report = s.solve_constraints().expect("solve");
    let pos = s.get_point(&p).expect("p");
    let dist = (pos.x * pos.x + pos.y * pos.y).sqrt();
    assert!(
        dist >= 5.0 - 1e-6,
        "MinDistance(5) must push the point out to at least the bound, got {dist}"
    );
    assert!(
        report.violations.is_empty(),
        "an enforced, satisfiable inequality must not be reported violated: {:?}",
        report.violations
    );
}

#[test]
fn red_max_distance_pulls_a_too_far_point_in_to_the_bound() {
    let s = fresh("slice6_max_distance_active");
    let anchor = s.add_point(Point2d::new(0.0, 0.0));
    fix_point(&s, &anchor);
    let p = s.add_point(Point2d::new(8.0, 0.0));
    s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(p)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::MaxDistance(5.0),
        vec![EntityRef::Point(anchor), EntityRef::Point(p)],
    ));

    let report = s.solve_constraints().expect("solve");
    let pos = s.get_point(&p).expect("p");
    let dist = (pos.x * pos.x + pos.y * pos.y).sqrt();
    assert!(
        dist <= 5.0 + 1e-6,
        "MaxDistance(5) must pull the point in to at most the bound, got {dist}"
    );
    assert!(report.violations.is_empty(), "{:?}", report.violations);
}

#[test]
fn red_satisfied_inequality_is_inactive_moves_nothing_and_is_not_redundant() {
    let s = fresh("slice6_inactive_inequality");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(7.0, 0.0));
    fix_point(&s, &a);
    fix_point(&s, &b);
    s.add_constraint(required_dim(
        DimensionalConstraint::MinDistance(5.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.is_empty(),
        "a satisfied inequality must be inert: {:?}",
        report.violations
    );
    let (pa, pb) = (s.get_point(&a).expect("a"), s.get_point(&b).expect("b"));
    assert_eq!((pa.x, pa.y), (0.0, 0.0));
    assert_eq!((pb.x, pb.y), (7.0, 0.0));

    // The certificate must NOT misclassify an inactive inequality
    // (zero residual, zero Jacobian row) as a redundant constraint —
    // it is simply satisfied and inactive.
    let cert = s.certify();
    let fact = cert
        .constraint_facts
        .iter()
        .find(|f| {
            matches!(
                f.constraint_type,
                geometry_engine::sketch2d::ConstraintType::Dimensional(
                    DimensionalConstraint::MinDistance(_)
                )
            )
        })
        .expect("MinDistance fact present");
    assert!(fact.satisfied, "inactive inequality is satisfied");
    assert_ne!(
        fact.role,
        ConstraintRole::Redundant,
        "an inactive inequality is NOT a removable duplicate"
    );
    assert_ne!(fact.role, ConstraintRole::Conflicting);
    assert!(cert.witnesses.is_empty(), "{:?}", cert.witnesses);
}

#[test]
fn red_unsatisfiable_inequality_between_fixed_points_stays_violated() {
    // Both points fixed at distance 1 with MinDistance(5): nothing can
    // move, the one-sided residual is irreducible — the solver must
    // NOT claim a clean solve.
    let s = fresh("slice6_inequality_unsatisfiable");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(1.0, 0.0));
    fix_point(&s, &a);
    fix_point(&s, &b);
    let cid = s.add_constraint(required_dim(
        DimensionalConstraint::MinDistance(5.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        report
            .violations
            .iter()
            .any(|(id, r)| *id == cid && *r > 1.0),
        "a violated, unsatisfiable inequality must surface with its true \
         residual (bound - distance = 4): {:?}",
        report.violations
    );
    assert!(!report.converged(), "status: {:?}", report.status);
}

// ── Offset + OffsetDistance: op-created offset pairs ────────────────

#[test]
fn red_offset_pair_of_lines_is_maintained_by_the_two_constraints() {
    let s = fresh("slice6_offset_lines");
    // Source segment pinned along X from (0,0) to (10,0).
    let s1 = s.add_point(Point2d::new(0.0, 0.0));
    let s2 = s.add_point(Point2d::new(10.0, 0.0));
    fix_point(&s, &s1);
    fix_point(&s, &s2);
    let src = s.add_line(s1, s2).expect("source line");
    // Offset line, deliberately perturbed off the true offset.
    let b1 = s.add_point(Point2d::new(0.5, 1.7));
    let b2 = s.add_point(Point2d::new(9.3, 2.4));
    let off = s.add_line(b1, b2).expect("offset line");

    s.add_constraint(high_geo(
        GeometricConstraint::Offset,
        vec![EntityRef::Line(src), EntityRef::Line(off)],
    ));
    s.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::OffsetDistance(2.0),
        vec![EntityRef::Line(src), EntityRef::Line(off)],
        ConstraintPriority::High,
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.is_empty(),
        "offset pair must be satisfiable: {:?}",
        report.violations
    );
    let (p1, p2) = (s.get_point(&b1).expect("b1"), s.get_point(&b2).expect("b2"));
    // Correspondence: endpoints displaced PERPENDICULAR to the source.
    assert!(p1.x.abs() < 1e-6, "b1.x should align with s1.x: {p1:?}");
    assert!((p2.x - 10.0).abs() < 1e-6, "b2.x should align: {p2:?}");
    // Gap: both endpoints exactly 2.0 off the source carrier (the +Y
    // side — the initial guess's basin).
    assert!((p1.y - 2.0).abs() < 1e-6, "b1 gap: {p1:?}");
    assert!((p2.y - 2.0).abs() < 1e-6, "b2 gap: {p2:?}");

    // DOF accounting: 4 free DOFs (b1, b2), Offset removes 3, and
    // OffsetDistance removes 1 → structurally fully constrained.
    let dof = s.analyze_dofs();
    assert_eq!(
        dof.status,
        DofStatus::FullyConstrained,
        "offset(3) + offset_distance(1) must pin the 4 endpoint DOFs: {dof:?}"
    );
}

#[test]
fn red_offset_pair_of_circles_stays_concentric_with_the_radial_gap() {
    let s = fresh("slice6_offset_circles");
    let src = s.add_circle(Point2d::new(5.0, 5.0), 3.0).expect("source");
    // Pin the source: center + radius.
    s.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(5.0),
        vec![EntityRef::Circle(src)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(5.0),
        vec![EntityRef::Circle(src)],
    ));
    s.add_constraint(required_dim(
        DimensionalConstraint::Radius(3.0),
        vec![EntityRef::Circle(src)],
    ));
    // Offset circle: perturbed center, wrong radius.
    let off = s.add_circle(Point2d::new(5.4, 4.6), 2.0).expect("offset");
    s.add_constraint(high_geo(
        GeometricConstraint::Offset,
        vec![EntityRef::Circle(src), EntityRef::Circle(off)],
    ));
    s.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::OffsetDistance(1.5),
        vec![EntityRef::Circle(src), EntityRef::Circle(off)],
        ConstraintPriority::High,
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(report.violations.is_empty(), "{:?}", report.violations);
    let center = s.circle_center_position(&off).expect("center");
    assert!(
        (center.x - 5.0).abs() < 1e-6 && (center.y - 5.0).abs() < 1e-6,
        "offset circle must be concentric with its source: {center:?}"
    );
    let r = s.circles().get(&off).expect("off").circle.radius;
    assert!(
        ((3.0 - r).abs() - 1.5).abs() < 1e-6,
        "radial gap |r_src - r_off| must equal 1.5, got r_off = {r}"
    );
}

// ── MultiTangent: one line tangent to N curves at once ──────────────

#[test]
fn red_multitangent_line_lands_tangent_to_both_circles() {
    let s = fresh("slice6_multitangent");
    // Two pinned equal circles — the classic belt line fixture.
    let c1 = s.add_circle(Point2d::new(0.0, 0.0), 2.0).expect("c1");
    let c2 = s.add_circle(Point2d::new(10.0, 0.0), 2.0).expect("c2");
    for (c, x) in [(c1, 0.0), (c2, 10.0)] {
        s.add_constraint(required_dim(
            DimensionalConstraint::XCoordinate(x),
            vec![EntityRef::Circle(c)],
        ));
        s.add_constraint(required_dim(
            DimensionalConstraint::YCoordinate(0.0),
            vec![EntityRef::Circle(c)],
        ));
        s.add_constraint(required_dim(
            DimensionalConstraint::Radius(2.0),
            vec![EntityRef::Circle(c)],
        ));
    }
    // Free belt line above both circles.
    let p1 = s.add_point(Point2d::new(0.0, 3.5));
    let p2 = s.add_point(Point2d::new(10.0, 3.5));
    let line = s.add_line(p1, p2).expect("belt line");
    let cid = s.add_constraint(high_geo(
        GeometricConstraint::MultiTangent,
        vec![
            EntityRef::Line(line),
            EntityRef::Circle(c1),
            EntityRef::Circle(c2),
        ],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        !report.violations.iter().any(|(id, _)| *id == cid),
        "MultiTangent must be enforced, not refused: {:?}",
        report.violations
    );
    // Verify true tangency: perpendicular distance from each center to
    // the line carrier equals the radius.
    let (a, b) = (s.get_point(&p1).expect("p1"), s.get_point(&p2).expect("p2"));
    let dir = ((b.x - a.x), (b.y - a.y));
    let len = (dir.0 * dir.0 + dir.1 * dir.1).sqrt();
    let perp = |cx: f64, cy: f64| ((cx - a.x) * dir.1 - (cy - a.y) * dir.0).abs() / len;
    assert!(
        (perp(0.0, 0.0) - 2.0).abs() < 1e-6,
        "line-c1 tangency: {}",
        perp(0.0, 0.0)
    );
    assert!(
        (perp(10.0, 0.0) - 2.0).abs() < 1e-6,
        "line-c2 tangency: {}",
        perp(10.0, 0.0)
    );
}

// ── The refuse set that REMAINS refused (documented, pinned) ────────

#[test]
fn remaining_three_variants_still_refuse_honestly() {
    // CurvatureExtremum, ContactConstraint, MomentOfInertia stay
    // honest-refuse (disposition table in the slice-6 report): each
    // removes zero DOF, is not numerically enforced, and surfaces an
    // irreducible violation instead of a silent zero.
    let s = fresh("slice6_refuse_survivors");
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(1.0, 0.0));
    fix_point(&s, &a);
    fix_point(&s, &b);

    let refusals = [
        Constraint::new_geometric(
            GeometricConstraint::CurvatureExtremum,
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::High,
        ),
        Constraint::new_geometric(
            GeometricConstraint::ContactConstraint,
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::High,
        ),
        Constraint::new_dimensional(
            DimensionalConstraint::MomentOfInertia(1.0),
            vec![EntityRef::Point(a)],
            ConstraintPriority::High,
        ),
    ];
    for con in refusals {
        assert_eq!(
            con.degrees_of_freedom_removed(),
            0,
            "{:?} must remove zero DOF",
            con.constraint_type
        );
        assert!(
            !con.constraint_type.is_numerically_enforced(),
            "{:?} must stay refuse-typed",
            con.constraint_type
        );
        let cid = s.add_constraint(con.clone());
        let report = s.solve_constraints().expect("solve");
        assert!(
            report.violations.iter().any(|(id, _)| *id == cid),
            "{:?} must surface an irreducible violation",
            con.constraint_type
        );
        assert!(!report.converged());
        s.remove_constraint(&cid);
    }
}

#[test]
fn offset_between_unsupported_kinds_refuses_instead_of_lying() {
    // Offset(line, circle) has no defined correspondence — the
    // evaluator must emit the irreducible refuse residual, never a
    // silent zero row that would fake satisfaction.
    let s = fresh("slice6_offset_unsupported_pair");
    let p1 = s.add_point(Point2d::new(0.0, 0.0));
    let p2 = s.add_point(Point2d::new(10.0, 0.0));
    fix_point(&s, &p1);
    fix_point(&s, &p2);
    let line = s.add_line(p1, p2).expect("line");
    let circle = s.add_circle(Point2d::new(5.0, 5.0), 2.0).expect("circle");
    let cid = s.add_constraint(high_geo(
        GeometricConstraint::Offset,
        vec![EntityRef::Line(line), EntityRef::Circle(circle)],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.iter().any(|(id, _)| *id == cid),
        "unsupported Offset pair must refuse: {:?}",
        report.violations
    );
    assert!(!report.converged());
}

#[test]
fn multitangent_without_a_leading_line_refuses() {
    let s = fresh("slice6_multitangent_shape_refuse");
    let c1 = s.add_circle(Point2d::new(0.0, 0.0), 2.0).expect("c1");
    let c2 = s.add_circle(Point2d::new(10.0, 0.0), 2.0).expect("c2");
    let cid = s.add_constraint(high_geo(
        GeometricConstraint::MultiTangent,
        vec![EntityRef::Circle(c1), EntityRef::Circle(c2)],
    ));
    let report = s.solve_constraints().expect("solve");
    assert!(
        report.violations.iter().any(|(id, _)| *id == cid),
        "curve-curve MultiTangent is out of the v1 envelope and must \
         refuse, not silently pass: {:?}",
        report.violations
    );
}
