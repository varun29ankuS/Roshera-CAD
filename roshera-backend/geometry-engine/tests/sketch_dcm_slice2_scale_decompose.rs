//! SKETCH-DCM campaign #45, Slice 2 — scale floor + connected-component
//! decomposition (phase 0).
//!
//! RED contract (written before the decomposition implementation):
//!
//! 1. **Scale floor** (spec §2.9 — "no test anywhere exercises >100
//!    constraints"): a realistic 300-constraint dimensioned plate
//!    (outline + ordinate-dimensioned bolt-hole pattern + reference
//!    dimension chain, generator in `tests/common/mod.rs`) must solve
//!    correctly within a wall-clock budget derived from measurement
//!    (see `SCALE_BUDGET_MS` for the derivation). The pre-slice
//!    one-big-system dense Newton path fails the budget by an order of
//!    magnitude: it forms and eliminates one joint normal-equation
//!    system over every parameter in the sketch even though the sketch
//!    is 78 independent constraint components.
//! 2. **Isolation**: solving one cluster must not perturb a disjoint,
//!    already-satisfied cluster (bitwise).
//! 3. **Aggregation honesty**: the decomposed path's external surface
//!    (status, violations, DOF accounting, refuse-constraint
//!    surfacing) stays indistinguishable from the whole-system path.

#![allow(clippy::float_cmp)]
// Reason for `#![allow(clippy::expect_used)]` / `unwrap_used` /
// `panic` — test-only file: failing loudly at the fixture site is the
// desired failure mode; the workspace deny lints target production
// code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

mod common;

use common::{generate_plate, generate_plate_salted, PlateSpec};
use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_solver::DragTarget;
use geometry_engine::sketch2d::Point2d;

/// Wall-clock budget (milliseconds of `SketchSolveReport::solve_time_ms`,
/// i.e. solver-internal time, generation excluded) for the LARGE
/// 300-constraint plate under the DEV test profile.
///
/// Derivation (measured on this machine, dev profile, 2026-07-15, via the
/// `#[ignore]`d `measure_scale_floor_reference_timings` harness below —
/// numbers recorded in `.superpowers/sdd/sketch-dcm-slice2-report.md`):
/// the decomposed-path expectation is the sum of solving each of the 78
/// constraint components as its own sketch (outline 8 DOF, chain 64 DOF,
/// 76 holes × 3 DOF) — measured **629.7 ms**. The budget is ~5× that
/// (3000 ms): the full-workspace gate co-schedules this binary with the
/// heaviest suites in the fleet and a 2× margin measured 1319 ms under
/// that load (gate run 2026-07-16) — the budget must absorb scheduler
/// noise WITHOUT losing its teeth, and 3000 ms is still 10× under the
/// dense one-big-system path, which measured **30521 ms** on the same
/// plate (35 Newton iterations, each paying a full 300-parameter
/// Jacobian + normal-equation elimination) — a genuine RED on the
/// pre-slice solver, not a regression pin.
const SCALE_BUDGET_MS: f64 = 3000.0;

/// Interactive budget for one drag re-solve on the LARGE plate (drag
/// preset: 30 iterations, 1e-6 tolerance). Same derivation session as
/// `SCALE_BUDGET_MS` — the dense path measured **27165 ms per drag
/// frame** (the drag pulls against Required pins, so the loop runs its
/// full 30 iterations at whole-system cost; the PlaneGCS #11498 failure
/// class). Decomposed, only tiny systems iterate; the budget mirrors
/// the scale budget's gate-load-tolerant derivation (5× piecewise,
/// 9× under the dense path).
const DRAG_BUDGET_MS: f64 = 3000.0;

/// Wall-clock-asserting tests take this lock so they never time a solve
/// while a sibling test (or the measurement harness) is saturating the
/// CPU — the budgets are per-solve latencies, not throughput-under-load
/// numbers. A poisoned lock (a failed budget assertion in another test)
/// must not mask this test's own verdict, hence `into_inner`.
static TIMING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn timing_guard() -> std::sync::MutexGuard<'static, ()> {
    TIMING_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fresh() -> Sketch {
    Sketch::new("dcm_slice2".to_string(), SketchAnchor::xy())
}

// ── Generator sanity: the spec sizes are what they claim ───────────

#[test]
fn plate_generator_hits_the_spec_sizes_and_is_fully_constrained() {
    for (spec, expected) in [
        (PlateSpec::SMALL, 30),
        (PlateSpec::MEDIUM, 100),
        (PlateSpec::LARGE, 300),
    ] {
        let plate = generate_plate(&spec);
        assert_eq!(plate.constraint_count, expected);
        let report = plate.sketch.analyze_dofs();
        assert!(
            report.is_fully_constrained(),
            "generated plate ({expected} constraints) must be structurally \
             fully constrained, got {:?} (free {}, removed {})",
            report.status,
            report.total_free_dofs,
            report.constraint_dofs_removed
        );
    }
}

// ── RED 1: the scale floor ──────────────────────────────────────────

#[test]
fn red_scale_floor_300_constraint_plate_solves_within_budget() {
    let _serial = timing_guard();
    let plate = generate_plate(&PlateSpec::LARGE);
    let report = plate.sketch.solve_constraints().expect("solve");

    // Correctness first: the budget means nothing if the solve is wrong.
    assert!(
        report.is_fully_constrained(),
        "300-constraint plate must converge fully constrained, got {:?} \
         (violations: {})",
        report.status,
        report.violations.len()
    );
    assert!(
        report.violations.is_empty(),
        "no residual may remain above tolerance: {:?}",
        report.violations
    );
    common::assert_plate_solved(&plate, 1e-6);

    // The scale floor itself.
    assert!(
        report.solve_time_ms < SCALE_BUDGET_MS,
        "scale floor: 300-constraint plate took {:.1} ms, budget {} ms \
         (~2× the measured piecewise/decomposed expectation — the \
         one-big-system dense path pays O(p³) per Newton iteration for a \
         sketch that is 78 independent components)",
        report.solve_time_ms,
        SCALE_BUDGET_MS
    );
}

#[test]
fn red_drag_on_large_plate_meets_interactive_budget() {
    let _serial = timing_guard();
    let plate = generate_plate(&PlateSpec::LARGE);
    // Converge once so the drag starts from a solved sketch, like a
    // real interactive session.
    let report = plate.sketch.solve_constraints().expect("initial solve");
    assert!(report.converged(), "setup solve: {:?}", report.status);

    // Drag a fully-dimensioned hole center: the Required ordinate
    // dimensions must win over the Low-priority drag pull, and the
    // frame must come back inside the interactive budget.
    let hole = plate.holes.last().expect("holes present");
    let target = Point2d::new(hole.cx + 3.0, hole.cy - 2.0);
    let drag_report = plate
        .sketch
        .solve_drag(EntityRef::Point(hole.center), DragTarget::Point(target))
        .expect("drag");

    let center = plate
        .sketch
        .get_point(&hole.center)
        .expect("hole center present");
    assert!(
        ((center.x - hole.cx).powi(2) + (center.y - hole.cy).powi(2)).sqrt() < 1e-3,
        "Required ordinate dimensions must hold the dragged hole center \
         at its dimensioned position, got {:?}",
        center
    );
    assert!(
        drag_report.solve_time_ms < DRAG_BUDGET_MS,
        "drag frame: {:.1} ms, budget {} ms — interactive drag dies first \
         at scale on the one-big-system path",
        drag_report.solve_time_ms,
        DRAG_BUDGET_MS
    );
}

// ── Isolation: disjoint clusters do not perturb each other ──────────

#[test]
fn solving_one_cluster_leaves_a_disjoint_satisfied_cluster_bitwise_untouched() {
    let sketch = fresh();

    // Cluster B: already exactly satisfied — pinned at its dimensions.
    let bx = 50.0;
    let by = 7.0;
    let b_point = sketch.add_point(Point2d::new(bx, by));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(bx),
        vec![EntityRef::Point(b_point)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(by),
        vec![EntityRef::Point(b_point)],
        ConstraintPriority::Required,
    ));

    // Cluster A: genuinely violated — Newton must move it.
    let a1 = sketch.add_point(Point2d::new(0.0, 0.0));
    let a2 = sketch.add_point(Point2d::new(3.0, 0.0));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(0.0),
        vec![EntityRef::Point(a1)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(a1)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(10.0),
        vec![EntityRef::Point(a1), EntityRef::Point(a2)],
        ConstraintPriority::Required,
    ));

    let report = sketch.solve_constraints().expect("solve");
    assert!(report.converged(), "solve failed: {:?}", report.status);

    // Cluster A did its work…
    let p1 = sketch.get_point(&a1).expect("a1");
    let p2 = sketch.get_point(&a2).expect("a2");
    let d = ((p1.x - p2.x).powi(2) + (p1.y - p2.y).powi(2)).sqrt();
    assert!(
        (d - 10.0).abs() < 1e-6,
        "cluster A distance not driven to its dimension: {d}"
    );

    // …and cluster B was not perturbed AT ALL: its residuals were
    // exactly zero, so its Newton step is exactly zero. Bitwise.
    let pb = sketch.get_point(&b_point).expect("b");
    assert_eq!(
        (pb.x, pb.y),
        (bx, by),
        "a solve of a disjoint cluster must not perturb an untouched, \
         already-satisfied cluster"
    );
}

// ── Aggregation honesty across components ───────────────────────────

#[test]
fn refuse_constraint_in_one_component_still_surfaces_globally() {
    // A hole cluster that solves fine + a disjoint pair carrying a
    // recognised-but-unenforced constraint (MomentOfInertia — the #19
    // honest-refuse contract; fixture migrated from MinDistance when
    // SKETCH-DCM #45 Slice 6 gave the inequalities real residuals).
    // The decomposed path must surface the irreducible violation
    // exactly like the whole-system path: the sketch is never
    // reported clean.
    let sketch = fresh();

    let hole_center = sketch.add_point(Point2d::new(10.2, 9.7));
    let circle = sketch
        .add_circle_centered(hole_center, 3.3)
        .expect("circle");
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(10.0),
        vec![EntityRef::Point(hole_center)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(10.0),
        vec![EntityRef::Point(hole_center)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Radius(3.0),
        vec![EntityRef::Circle(circle)],
        ConstraintPriority::Required,
    ));

    let q1 = sketch.add_point(Point2d::new(100.0, 0.0));
    let q2 = sketch.add_point(Point2d::new(105.0, 0.0));
    let refuse_id = sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::MomentOfInertia(2.0),
        vec![EntityRef::Point(q1), EntityRef::Point(q2)],
        ConstraintPriority::Required,
    ));

    let report = sketch.solve_constraints().expect("solve");

    // The refuse constraint's irreducible residual must appear in the
    // violations regardless of which component it lives in.
    assert!(
        report.violations.iter().any(|(id, _)| *id == refuse_id),
        "the honest-refuse MinDistance residual must survive \
         decomposition aggregation; violations: {:?}",
        report.violations
    );
    assert!(
        !report.converged(),
        "a sketch carrying an unenforceable constraint must never be \
         reported clean, got {:?}",
        report.status
    );

    // The enforceable cluster still solved.
    let c = sketch.get_point(&hole_center).expect("center");
    assert!(
        ((c.x - 10.0).powi(2) + (c.y - 10.0).powi(2)).sqrt() < 1e-6,
        "enforceable cluster must still solve: {:?}",
        c
    );
}

#[test]
fn under_constrained_components_sum_under_the_precedence_fold() {
    // Two disjoint clusters, NEITHER over-constrained: one fully
    // constrained, one with 2 free DOFs. SKETCH-DCM #45 slice H4
    // replaced the naive global-sum DOF comparison with a
    // per-component precedence fold (any component over-constrained
    // outranks any component under-constrained; see
    // `ConstraintSolver::check_constraint_count`). This fixture has no
    // over-constrained component, so the fold reports `UnderConstrained`
    // with the sum of the per-component deficits (0 + 2 = 2) — the
    // SAME number the old global-sum comparison happened to produce
    // for this particular fixture, because nothing here cancels. This
    // test does NOT exercise the cancellation defect itself (see
    // `over_and_under_components_never_cancel_to_fully_constrained`
    // below for that); it pins that the fold's arithmetic still
    // matches the pre-slice number in the non-cancelling case.
    let sketch = fresh();

    let pinned = sketch.add_point(Point2d::new(1.0, 2.0));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(1.0),
        vec![EntityRef::Point(pinned)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(2.0),
        vec![EntityRef::Point(pinned)],
        ConstraintPriority::Required,
    ));

    let free_a = sketch.add_point(Point2d::new(30.0, 0.0));
    let free_b = sketch.add_point(Point2d::new(37.0, 0.0));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(free_a), EntityRef::Point(free_b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(free_a)],
        ConstraintPriority::Required,
    ));

    let report = sketch.solve_constraints().expect("solve");
    assert_eq!(
        report.degrees_of_freedom(),
        Some(2),
        "no component here is over-constrained, so the fold reports \
         UnderConstrained with the summed per-component deficit, got {:?}",
        report.status
    );
    // …and the residuals were still all driven under tolerance.
    assert!(
        report.violations.is_empty(),
        "under-constrained but satisfiable: {:?}",
        report.violations
    );
}

#[test]
fn over_and_under_components_never_cancel_to_fully_constrained() {
    // THE H4 headline defect, exercised through `Sketch::solve_constraints`
    // (site 2: `ConstraintSolver::check_constraint_count`, the
    // `SolverResult.status` / `SketchSolveReport.status` path — the
    // analogous cancellation to `analyze_dofs`'s own fold, fixed the
    // same way).
    //
    // Component A: points a=(1,2), b=(6,2) + Distance(5.0) [consistent:
    // they really are 5.0 apart] + X(a)=1 + Y(a)=2 + X(b)=6 + Y(b)=2.
    // Free DOFs = 4, removed = 5 → over-constrained by 1 (a REDUNDANT,
    // fully satisfiable surplus — Newton converges this component to
    // zero residual).
    //
    // Component B: lone point c=(3,0) with only Y(c)=0. Free DOFs = 2,
    // removed = 1 → under-constrained by 1.
    //
    // Global tallies: free 4+2=6, removed 5+1=6 — the pre-H4 global
    // comparison cancels to `Equal`, so `check_constraint_count`
    // returned `None` and `solve()` fell through to Newton's own
    // outcome. Because both components are individually SATISFIABLE
    // (component A's surplus is redundant, not conflicting; component
    // B has no constraint contradicting its free coordinate), Newton
    // actually converges — so the pre-fix report was `Converged`, not
    // even the "balanced" `FullyConstrained`-flavoured lie `analyze_dofs`
    // told, but still wrong: a genuinely over-constrained component
    // must be reported as such regardless of whether its surplus
    // happens to be consistent.
    let sketch = fresh();
    let a = sketch.add_point(Point2d::new(1.0, 2.0));
    let b = sketch.add_point(Point2d::new(6.0, 2.0));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(a), EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(1.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(2.0),
        vec![EntityRef::Point(a)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::XCoordinate(6.0),
        vec![EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(2.0),
        vec![EntityRef::Point(b)],
        ConstraintPriority::Required,
    ));

    let c = sketch.add_point(Point2d::new(3.0, 0.0));
    sketch.add_constraint(Constraint::new_dimensional(
        DimensionalConstraint::YCoordinate(0.0),
        vec![EntityRef::Point(c)],
        ConstraintPriority::Required,
    ));

    let report = sketch.solve_constraints().expect("solve");
    assert_eq!(
        report.conflicting_constraints(),
        Some(1),
        "the over-constrained component must outrank the \
         under-constrained one under the precedence fold, got status {:?}",
        report.status
    );
    assert!(
        !report.is_fully_constrained(),
        "must never present the cancellation as fully constrained \
         (Converged with 0 remaining DOF), got {:?}",
        report.status
    );
}

// ── Dense ≡ decomposed equivalence ──────────────────────────────────

#[test]
fn dense_and_decomposed_paths_agree_on_the_medium_plate() {
    use geometry_engine::sketch2d::constraint_solver::EntityUpdate;
    use geometry_engine::sketch2d::sketch_solver::{build_solver, SolveOptions};

    let plate = generate_plate(&PlateSpec::MEDIUM);

    let mut dense = build_solver(&plate.sketch, SolveOptions::default()).expect("dense solver");
    dense.set_decomposition_enabled(false);
    let mut decomposed =
        build_solver(&plate.sketch, SolveOptions::default()).expect("decomposed solver");
    assert!(decomposed.decomposition_enabled(), "default must be ON");

    let dense_result = dense.solve();
    let decomposed_result = decomposed.solve();

    // Verdict equivalence.
    assert!(
        matches!(
            (dense_result.status, decomposed_result.status),
            (
                geometry_engine::sketch2d::constraint_solver::SolverStatus::Converged { .. },
                geometry_engine::sketch2d::constraint_solver::SolverStatus::Converged { .. },
            )
        ),
        "status kinds must match: dense {:?} vs decomposed {:?}",
        dense_result.status,
        decomposed_result.status
    );
    assert!(dense_result.violations.is_empty());
    assert!(decomposed_result.violations.is_empty());

    // Solution equivalence: every entity update agrees to fp-comparable
    // tolerance (both paths converge the same fully-constrained system
    // to 1e-10, so solutions match far tighter than the 1e-8 asserted).
    assert_eq!(
        dense_result.entity_updates.len(),
        decomposed_result.entity_updates.len()
    );
    for (entity, dense_update) in &dense_result.entity_updates {
        let decomposed_update = decomposed_result
            .entity_updates
            .get(entity)
            .expect("entity present in both result sets");
        match (dense_update, decomposed_update) {
            (EntityUpdate::Point(a), EntityUpdate::Point(b)) => {
                assert!(
                    (a.x - b.x).abs() < 1e-8 && (a.y - b.y).abs() < 1e-8,
                    "point diverged between paths: {:?} vs {:?}",
                    a,
                    b
                );
            }
            (EntityUpdate::Circle(ca, ra), EntityUpdate::Circle(cb, rb)) => {
                assert!(
                    (ca.x - cb.x).abs() < 1e-8
                        && (ca.y - cb.y).abs() < 1e-8
                        && (ra - rb).abs() < 1e-8,
                    "circle diverged between paths: ({:?}, {ra}) vs ({:?}, {rb})",
                    ca,
                    cb
                );
            }
            (a, b) => panic!("update kind mismatch for {entity:?}: {a:?} vs {b:?}"),
        }
    }
}

// ── Determinism ─────────────────────────────────────────────────────

#[test]
fn independently_generated_plates_solve_to_identical_dimensioned_geometry() {
    // Two fresh MEDIUM plates (different entity UUIDs, different jitter
    // salt — so different DashMap iteration orders and different
    // component discovery inputs) must both land every dimensioned
    // target. Component ordering or map iteration order must never
    // leak into the solved geometry.
    for salt in [1_usize, 2] {
        let plate = generate_plate_salted(&PlateSpec::MEDIUM, salt);
        let report = plate.sketch.solve_constraints().expect("solve");
        assert!(
            report.is_fully_constrained(),
            "salt {salt}: {:?}",
            report.status
        );
        common::assert_plate_solved(&plate, 1e-6);
    }
}

#[test]
fn repeated_solves_of_the_same_sketch_are_stable() {
    let plate = generate_plate(&PlateSpec::SMALL);
    let first = plate.sketch.solve_constraints().expect("first solve");
    assert!(first.is_fully_constrained(), "{:?}", first.status);
    common::assert_plate_solved(&plate, 1e-6);
    // Snapshot solved geometry, solve again from the converged state:
    // nothing may move (zero residual ⇒ zero step, every component).
    let snapshot: Vec<(f64, f64)> = plate
        .corners
        .iter()
        .map(|(id, _, _)| {
            let p = plate.sketch.get_point(id).expect("corner");
            (p.x, p.y)
        })
        .collect();
    let second = plate.sketch.solve_constraints().expect("second solve");
    assert!(second.is_fully_constrained(), "{:?}", second.status);
    for ((id, _, _), (x, y)) in plate.corners.iter().zip(snapshot) {
        let p = plate.sketch.get_point(id).expect("corner");
        assert_eq!(
            (p.x, p.y),
            (x, y),
            "re-solving a converged sketch must be a bitwise no-op"
        );
    }
}

// ── Measurement harness (documents the budget derivation) ──────────

/// Not part of the gate — run explicitly with `--ignored --nocapture`
/// to reproduce the numbers behind `SCALE_BUDGET_MS` / `DRAG_BUDGET_MS`.
/// Records: (a) the full 300-constraint solve through the shipped
/// bridge path, (b) the piecewise sum — every constraint component
/// solved as its own sketch — which is the decomposed-path expectation
/// the slice contract derives the budget from.
#[test]
#[ignore = "measurement harness: budget derivation, not a gate test"]
fn measure_scale_floor_reference_timings() {
    let _serial = timing_guard();
    let spec = PlateSpec::LARGE;

    // (a) full plate through the shipped solve path.
    let plate = generate_plate(&spec);
    let report = plate.sketch.solve_constraints().expect("solve");
    println!(
        "full {}-constraint plate: solve_time_ms = {:.2}, status = {:?}, violations = {}",
        plate.constraint_count,
        report.solve_time_ms,
        report.status,
        report.violations.len()
    );

    // (b) piecewise: each component as its own sketch.
    let mut piecewise_ms = 0.0;
    let outline_sketch = Sketch::new("outline".to_string(), SketchAnchor::xy());
    common::add_outline(&outline_sketch, 0);
    let r = outline_sketch.solve_constraints().expect("outline");
    piecewise_ms += r.solve_time_ms;

    let chain_sketch = Sketch::new("chain".to_string(), SketchAnchor::xy());
    common::add_chain(&chain_sketch, spec.chain_points, 0);
    let r = chain_sketch.solve_constraints().expect("chain");
    piecewise_ms += r.solve_time_ms;

    for i in 0..spec.holes {
        let hole_sketch = Sketch::new(format!("hole{i}"), SketchAnchor::xy());
        common::add_hole(&hole_sketch, i, 0);
        let r = hole_sketch.solve_constraints().expect("hole");
        piecewise_ms += r.solve_time_ms;
    }
    println!(
        "piecewise sum over {} components (decomposed-path expectation): {:.2} ms",
        spec.expected_components(),
        piecewise_ms
    );

    // Drag reference on a fresh solved plate.
    let plate = generate_plate(&spec);
    let _ = plate.sketch.solve_constraints().expect("setup solve");
    let hole = plate.holes.last().expect("holes");
    let drag = plate
        .sketch
        .solve_drag(
            EntityRef::Point(hole.center),
            DragTarget::Point(Point2d::new(hole.cx + 3.0, hole.cy - 2.0)),
        )
        .expect("drag");
    println!(
        "drag frame on solved plate: solve_time_ms = {:.2}",
        drag.solve_time_ms
    );
}
