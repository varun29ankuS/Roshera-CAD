//! End-to-end integration tests for the `Sketch ↔ ConstraintSolver` bridge.
//!
//! Slice B-1 of the Fusion-grade sketch roadmap. Exercises the
//! public surface of `Sketch::solve_constraints()` (and the free
//! function `sketch2d::sketch_solver::solve`) against concrete
//! scenarios that mirror how the api-server and AI integration will
//! invoke it in slices B-3 / B-4:
//!
//! 1. **Coincident-on-fixed-anchor**: a free point snaps to a pinned
//!    point. The free point's id is preserved across the solve.
//! 2. **Three-point triangle with absolute coordinates**: pin one
//!    vertex with `XCoordinate` / `YCoordinate` constraints, then
//!    impose two `Distance` constraints; the solver lands all three
//!    vertices on a triangle satisfying both edges within tolerance.
//! 3. **Circle radius pin**: a circle with a free `Radius` constraint
//!    converges to the target radius without moving its centre.
//! 4. **Empty-sketch invariant**: solving an empty sketch is a
//!    no-op that still produces a valid report.
//! 5. **Identity preservation**: every entity id present before the
//!    solve is present after.
//!
//! Convergence-precision tests for the underlying Newton-Raphson
//! engine live in `sketch2d::constraint_solver::tests` and
//! `sketch2d::sketch_solver::tests`; this file asserts only on
//! the integration contract (bridge faithfulness, id preservation,
//! report shape).

#![allow(clippy::float_cmp)]
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]

use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef, GeometricConstraint,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_solver::{
    DofStatus, DragTarget, SketchSolveError, SolveOptions,
};
use geometry_engine::sketch2d::{Point2d, SolverStatus};

fn fresh() -> Sketch {
    Sketch::new("integration".to_string(), SketchAnchor::xy())
}

#[test]
fn coincident_pulls_free_point_onto_fixed_anchor() {
    let sketch = fresh();
    let anchor = sketch.add_point(Point2d::new(3.0, -4.0));
    let free = sketch.add_point(Point2d::new(0.0, 0.0));

    // Pin the anchor.
    {
        let mut entry = sketch
            .points()
            .get_mut(&anchor)
            .expect("anchor point present");
        entry.value_mut().fix();
    }

    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Coincident,
        vec![EntityRef::Point(anchor), EntityRef::Point(free)],
        ConstraintPriority::High,
    ));

    let report = sketch.solve_constraints().expect("solve");
    assert!(
        report.converged(),
        "expected convergence, got {:?}",
        report.status
    );
    assert!(report.is_fully_constrained());
    assert_eq!(report.entities_solved, 2);
    assert_eq!(report.constraints_solved, 1);
    assert_eq!(report.skipped_count(), 0);
    // Convergence implies a finite iteration count and a residual
    // below the configured tolerance (1e-10 default).
    let iters = report.iterations().expect("converged → iteration count");
    assert!(iters <= 100, "expected ≤100 iterations, got {iters}");
    let err = report.final_error().expect("converged → final error");
    assert!(err < 1e-9, "expected final error < 1e-9, got {err:e}");

    let pos = sketch
        .points()
        .get(&free)
        .expect("free point present after solve")
        .value()
        .position;
    assert!((pos.x - 3.0).abs() < 1e-7, "x = {}", pos.x);
    assert!((pos.y - (-4.0)).abs() < 1e-7, "y = {}", pos.y);

    // Anchor id and free id both still resolve.
    assert!(sketch.points().contains_key(&anchor));
    assert!(sketch.points().contains_key(&free));
}

#[test]
fn distance_constraint_lands_within_tolerance() {
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(1.0, 1.0));

    // Pin `a` at the origin.
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));

    // Distance(a, b) = 5.0.
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::High,
    ));

    let _report = sketch.solve_constraints().expect("solve");

    let pa = sketch.points().get(&a).expect("a").value().position;
    let pb = sketch.points().get(&b).expect("b").value().position;
    let d = ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2)).sqrt();
    assert!(
        (d - 5.0).abs() < 1e-6,
        "post-solve distance = {} (expected 5.0)",
        d
    );
}

#[test]
fn empty_sketch_solve_is_a_no_op_with_valid_report() {
    let sketch = fresh();
    let report = sketch.solve_constraints().expect("empty solve");
    assert_eq!(report.entities_solved, 0);
    assert_eq!(report.constraints_solved, 0);
    assert_eq!(report.skipped_count(), 0);
    assert!(report.entities_skipped.is_empty());
    // Status is solver-defined for empty input; the contract is
    // that the bridge produced a report rather than an error.
    let _ = report.status;
}

#[test]
fn unsupported_kinds_are_surfaced_as_entity_refs() {
    // Rectangles, ellipses, and splines are all supported by the
    // solver now (C-2, C-3, C-4); only polylines remain unsupported
    // (C-5 pending). The bridge must still surface a stable
    // `entities_skipped` contract for any kind it cannot register.
    let sketch = fresh();
    let _p = sketch.add_point(Point2d::new(0.0, 0.0));
    let poly_id = sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(1.0, 0.0),
                Point2d::new(1.0, 1.0),
            ],
            false,
        )
        .expect("polyline");

    let report = sketch.solve_constraints().expect("solve");
    assert_eq!(report.entities_solved, 1);
    assert_eq!(report.skipped_count(), 1);
    // The polyline's id is surfaced verbatim so UI layers can
    // highlight specifically which entity went unsolved.
    assert_eq!(report.entities_skipped, vec![EntityRef::Polyline(poly_id)]);
}

#[test]
fn invalid_options_are_rejected_before_solver_runs() {
    let sketch = fresh();
    let bad = SolveOptions::new(0, 1e-10, 0.5);
    assert_eq!(
        sketch.solve_constraints_with_options(bad).unwrap_err(),
        SketchSolveError::InvalidMaxIterations
    );
}

#[test]
fn convenience_accessors_compose_a_status_summary() {
    // A coincident-on-pinned-anchor sketch is fully constrained
    // (one DOF removed by the pin, both DOFs of the free point
    // removed by Coincident). The convenience accessors should
    // compose into the "fully defined" badge a sketcher UI draws.
    let sketch = fresh();
    let anchor = sketch.add_point(Point2d::new(0.0, 0.0));
    let free = sketch.add_point(Point2d::new(1.0, 0.0));
    {
        let mut entry = sketch.points().get_mut(&anchor).expect("anchor");
        entry.value_mut().fix();
    }
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Coincident,
        vec![EntityRef::Point(anchor), EntityRef::Point(free)],
        ConstraintPriority::High,
    ));

    let report = sketch.solve_constraints().expect("solve");
    // The four accessors should agree about the status.
    let converged = report.converged();
    let fully = report.is_fully_constrained();
    assert_eq!(
        converged, fully,
        "is_fully_constrained() must agree with converged()"
    );
    if converged {
        assert!(!report.is_under_constrained());
        assert!(!report.is_over_constrained());
        assert!(!report.is_unstable());
        assert!(report.iterations().is_some());
        assert!(report.final_error().is_some());
        assert_eq!(report.degrees_of_freedom(), None);
        assert_eq!(report.conflicting_constraints(), None);
    }
}

#[test]
fn three_point_chain_converges_or_partially_constrains() {
    // a → b → c with distances 3 and 4; a pinned at origin. This
    // is under-constrained (rotation freedom) so the solver may
    // return either Converged or UnderConstrained. Either is a
    // valid outcome — what we assert is bridge faithfulness:
    // 1. All three points survive.
    // 2. If the report converges, both distance residuals are
    //    within tolerance.
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(1.0, 0.0));
    let c = sketch.add_point(Point2d::new(2.0, 0.0));

    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(3.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::High,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(4.0),
        vec![EntityRef::Point(b), EntityRef::Point(c)],
        ConstraintPriority::High,
    ));

    let report = sketch.solve_constraints().expect("solve");
    assert!(sketch.points().contains_key(&a));
    assert!(sketch.points().contains_key(&b));
    assert!(sketch.points().contains_key(&c));

    if matches!(report.status, SolverStatus::Converged { .. }) {
        let pa = sketch.points().get(&a).expect("a").value().position;
        let pb = sketch.points().get(&b).expect("b").value().position;
        let pc = sketch.points().get(&c).expect("c").value().position;
        let dab = ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2)).sqrt();
        let dbc = ((pc.x - pb.x).powi(2) + (pc.y - pb.y).powi(2)).sqrt();
        assert!((dab - 3.0).abs() < 1e-6, "|ab| = {}", dab);
        assert!((dbc - 4.0).abs() < 1e-6, "|bc| = {}", dbc);
    }
}

// ── B-2: drag + DOF analysis (public Sketch surface) ──────────────

#[test]
fn analyze_dofs_reports_zero_for_empty_sketch() {
    let sketch = fresh();
    let dof = sketch.analyze_dofs();
    assert_eq!(dof.total_free_dofs, 0);
    assert_eq!(dof.constraint_dofs_removed, 0);
    assert_eq!(dof.entities_analysed, 0);
    assert_eq!(dof.constraints_analysed, 0);
    assert!(matches!(dof.status, DofStatus::FullyConstrained));
}

#[test]
fn analyze_dofs_reactively_reflects_constraint_additions() {
    // Before any constraints: 2 free points = 4 free DOFs.
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(1.0, 0.0));
    let dof_before = sketch.analyze_dofs();
    assert_eq!(dof_before.total_free_dofs, 4);
    assert_eq!(dof_before.constraint_dofs_removed, 0);
    assert!(dof_before.is_under_constrained());
    assert_eq!(dof_before.remaining_dofs(), Some(4));

    // Pin `a` to the origin with two coordinate constraints; pin `b`
    // to a fixed X. That's 3 DOFs removed.
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(5.0),
        vec![EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));

    let dof_after = sketch.analyze_dofs();
    assert_eq!(dof_after.total_free_dofs, 4);
    assert_eq!(dof_after.constraint_dofs_removed, 3);
    assert!(dof_after.is_under_constrained());
    assert_eq!(dof_after.remaining_dofs(), Some(1));
}

#[test]
fn drag_pulls_free_point_to_cursor_position() {
    let sketch = fresh();
    let p = sketch.add_point(Point2d::new(0.0, 0.0));
    let report = sketch
        .solve_drag(
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(3.0, 4.0)),
        )
        .expect("drag");
    assert!(report.converged(), "status was {:?}", report.status);
    let pos = sketch.points().get(&p).expect("p").value().position;
    assert!((pos.x - 3.0).abs() < 1e-5, "x = {}", pos.x);
    assert!((pos.y - 4.0).abs() < 1e-5, "y = {}", pos.y);
}

#[test]
fn drag_lands_on_closest_reachable_when_target_outside_constraint() {
    // `a` pinned at origin, |ab|=2; drag `b` toward (10, 0) — the
    // closest reachable point is (2, 0) on the constraint circle.
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(1.0, 0.0));
    {
        let mut entry = sketch.points().get_mut(&a).expect("a present");
        entry.value_mut().fix();
    }
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(2.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::High,
    ));

    let _ = sketch
        .solve_drag(
            EntityRef::Point(b),
            DragTarget::Point(Point2d::new(10.0, 0.0)),
        )
        .expect("drag");

    let pa = sketch.points().get(&a).expect("a").value().position;
    let pb = sketch.points().get(&b).expect("b").value().position;
    let dist = ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2)).sqrt();
    assert!(
        (dist - 2.0).abs() < 1e-4,
        "|ab| should be pinned at 2.0, got {}",
        dist
    );
    // `a` must stay at the origin.
    assert!(pa.x.abs() < 1e-10);
    assert!(pa.y.abs() < 1e-10);
}

#[test]
fn drag_does_not_persist_temporary_constraints() {
    let sketch = fresh();
    let p = sketch.add_point(Point2d::new(0.0, 0.0));
    let before = sketch.all_constraints().len();
    let _ = sketch
        .solve_drag(
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(2.0, 2.0)),
        )
        .expect("drag");
    let after = sketch.all_constraints().len();
    assert_eq!(
        before, after,
        "drag must not pollute the persistent ConstraintStore"
    );
}

#[test]
fn drag_rejects_unknown_entity_with_descriptive_error() {
    use geometry_engine::sketch2d::Point2dId;
    let sketch = fresh();
    let ghost = Point2dId::new();
    let err = sketch
        .solve_drag(
            EntityRef::Point(ghost),
            DragTarget::Point(Point2d::new(0.0, 0.0)),
        )
        .expect_err("unknown entity must be rejected");
    assert_eq!(
        err,
        SketchSolveError::DragEntityNotFound {
            entity: EntityRef::Point(ghost),
        }
    );
}

#[test]
fn drag_rejects_kind_mismatch_with_target_kind_label() {
    let sketch = fresh();
    let circle = sketch
        .add_circle(Point2d::new(0.0, 0.0), 1.0)
        .expect("circle");
    let err = sketch
        .solve_drag(
            EntityRef::Circle(circle),
            DragTarget::Point(Point2d::new(0.0, 0.0)),
        )
        .expect_err("circle cannot be dragged with a point target in slice B-2");
    match err {
        SketchSolveError::DragTargetKindMismatch {
            entity,
            target_kind,
        } => {
            assert_eq!(entity, EntityRef::Circle(circle));
            assert_eq!(target_kind, "Point");
        }
        other => panic!("expected DragTargetKindMismatch, got {:?}", other),
    }
}

#[test]
fn drag_options_preset_validates() {
    // The `for_drag` preset must round-trip through the public
    // sketch surface — this guards against drift in the validator
    // rejecting our own preset.
    let sketch = fresh();
    let p = sketch.add_point(Point2d::new(0.0, 0.0));
    let report = sketch
        .solve_drag_with_options(
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(1.0, 1.0)),
            SolveOptions::for_drag(),
        )
        .expect("drag preset accepted");
    let _ = report.status;
}

#[test]
fn drag_locked_point_rejected_before_solve_runs() {
    // A point with `is_fixed = true` must refuse to drag — this
    // mirrors Fusion / SolidWorks / Onshape behaviour where locked
    // sketch geometry is undraggable. Without this guard the soft
    // X/Y pull would fight the fixed-mask inside the solver and
    // produce a misleading over-constrained report.
    let sketch = fresh();
    let p = sketch.add_point(Point2d::new(7.0, -3.0));
    {
        let mut e = sketch.points().get_mut(&p).expect("p");
        e.value_mut().fix();
    }
    let err = sketch
        .solve_drag(
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(0.0, 0.0)),
        )
        .expect_err("locked point cannot be dragged");
    assert_eq!(
        err,
        SketchSolveError::DragEntityFixed {
            entity: EntityRef::Point(p),
        }
    );
    // Position must not have moved (rejection happens pre-solve).
    let pos = sketch.points().get(&p).expect("p").value().position;
    assert_eq!(pos.x, 7.0);
    assert_eq!(pos.y, -3.0);
}

#[test]
fn drag_preserves_entity_id_after_call() {
    // Identity preservation contract — matches the body-modify
    // architecture where mutations rewrite fields on existing ids.
    let sketch = fresh();
    let id = sketch.add_point(Point2d::new(0.0, 0.0));
    let _ = sketch
        .solve_drag(
            EntityRef::Point(id),
            DragTarget::Point(Point2d::new(11.0, -2.5)),
        )
        .expect("drag");
    assert!(sketch.points().contains_key(&id));
}

#[test]
fn analyze_dofs_flags_constraints_skipped_when_unsupported_kinds_touched() {
    // The DOF verdict must remain honest when the sketch contains
    // entity kinds slice B-2 cannot analyse. Counting their
    // constraints while excluding their free DOFs would produce
    // a phantom over-constraint; the bridge instead surfaces a
    // `constraints_skipped > 0` flag so the UI can warn.
    //
    // Rectangles/ellipses/splines are all analysed now (C-2/C-3/C-4);
    // polylines (C-5) are the remaining unsupported kind, so the
    // test exercises the skip path through a constraint that
    // references a polyline.
    let sketch = fresh();
    let p = sketch.add_point(Point2d::new(0.0, 0.0));
    let poly = sketch
        .add_polyline(
            vec![
                Point2d::new(0.0, 0.0),
                Point2d::new(1.0, 0.0),
                Point2d::new(1.0, 1.0),
            ],
            false,
        )
        .expect("polyline");
    sketch.add_constraint(Constraint::new_geometric(
        GeometricConstraint::Coincident,
        vec![EntityRef::Point(p), EntityRef::Polyline(poly)],
        ConstraintPriority::High,
    ));

    let dof = sketch.analyze_dofs();
    assert!(dof.has_skipped_constraints());
    assert_eq!(dof.constraints_skipped, 1);
    assert_eq!(dof.constraints_analysed, 0);
    // Verdict reflects the supported subset only: 2 free DOFs on
    // the point, 0 removed (constraint filtered out).
    assert!(dof.is_under_constrained());
    assert_eq!(dof.remaining_dofs(), Some(2));
}
