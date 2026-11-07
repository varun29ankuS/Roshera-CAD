// Reason: integration-test crate -- panicking (unwrap/expect/assert) is the
// test framework's failure mechanism; the workspace production deny stands.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! SK-C4: a solve's verdict is read from the Jacobian's rank, and a
//! count never masks a solve that did not converge.
//!
//! The solve status used to come from the plain DOF COUNT, which outranked
//! Newton's own outcome. A count cannot tell "two DOFs removed" from "one
//! DOF removed twice", so a free point carrying `XCoordinate(0)` twice read
//! fully constrained while its y slid freely; and a count that came out
//! under-constrained reported `UnderConstrained` over a solve that never
//! converged — which the dimension-edit route accepts as success.

use geometry_engine::sketch2d::{
    Constraint, ConstraintPriority, DimensionalConstraint, DofStatus, EntityRef, Point2d, Sketch,
    SketchAnchor, SolverStatus,
};

fn sketch() -> Sketch {
    Sketch::new("verdict".to_string(), SketchAnchor::xy())
}

fn dimension(value: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(value, entities, ConstraintPriority::Required)
}

#[test]
fn a_dof_removed_twice_does_not_read_fully_constrained() {
    let s = sketch();
    let p = s.add_point(Point2d::new(0.0, 3.0));
    s.add_constraint(dimension(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(p)],
    ));
    s.add_constraint(dimension(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(p)],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        !report.is_fully_constrained(),
        "y is untouched, so the point is not fully constrained: {:?}",
        report.status
    );
    assert_eq!(
        report.status,
        SolverStatus::UnderConstrained {
            degrees_of_freedom: 1
        },
        "one independent row on two parameters leaves one DOF"
    );
    assert!(
        report.converged(),
        "every constraint holds, so the sketch is solved"
    );
    let dof = report
        .dof
        .as_ref()
        .expect("the solve carries its DOF report");
    assert_eq!(dof.status, DofStatus::UnderConstrained { dofs: 1 });
    assert_eq!(dof.redundant.len(), 1, "one of the two rows is a duplicate");
}

#[test]
fn an_under_count_never_masks_a_solve_that_did_not_converge() {
    // Two free points, 4 parameters, 2 rows by count: under-constrained.
    // But the two rows ask for the same distance to be 5 and 7 — no
    // configuration satisfies both, so Newton cannot converge.
    let s = sketch();
    let a = s.add_point(Point2d::new(0.0, 0.0));
    let b = s.add_point(Point2d::new(6.0, 0.0));
    s.add_constraint(dimension(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    ));
    s.add_constraint(dimension(
        DimensionalConstraint::Distance(7.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        matches!(report.status, SolverStatus::NotConverged { .. }),
        "the residual stays above tolerance; a count must not report \
         this as UnderConstrained: {:?}",
        report.status
    );
    assert!(!report.converged(), "an unsatisfiable sketch is not solved");
    assert!(!report.violations.is_empty(), "the violated rows are named");
}

#[test]
fn a_fully_dimensioned_point_still_converges_fully_constrained() {
    // Control: two independent rows on two parameters.
    let s = sketch();
    let p = s.add_point(Point2d::new(1.0, 1.0));
    s.add_constraint(dimension(
        DimensionalConstraint::XCoordinate(4.0),
        vec![EntityRef::Point(p)],
    ));
    s.add_constraint(dimension(
        DimensionalConstraint::YCoordinate(-2.0),
        vec![EntityRef::Point(p)],
    ));

    let report = s.solve_constraints().expect("solve");
    assert!(
        report.is_fully_constrained(),
        "x and y are both pinned: {:?}",
        report.status
    );
    let dof = report
        .dof
        .as_ref()
        .expect("the solve carries its DOF report");
    assert_eq!(dof.status, DofStatus::FullyConstrained);
}
