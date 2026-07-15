//! SKETCH-DCM campaign #45, Slice 3 — rigid-cluster DR-plan
//! (Fudos-Hoffmann merge) + drag re-solve scoping.
//!
//! RED contract (recorded against the pre-slice solver — the plan
//! disabled via its default, which forces the exact Slice-2 path):
//!
//! 1. **Serial-cluster scaling** (the payoff RED): a 64-point
//!    dimension chain — ONE connected component, the topology that
//!    dominates the Slice-2 LARGE-plate solve — must solve within a
//!    budget derived from the serial per-cluster expectation (each
//!    point placed against the previous one is a tiny 2-DOF solve).
//!    The whole-component dense Newton pays O(p³) per iteration on
//!    all 128 parameters and misses the budget by an order of
//!    magnitude.
//! 2. **Drag re-solve scoping**: on a solved sketch, a drag pull must
//!    re-solve only the dragged entity's cluster chain — asserted
//!    STRUCTURALLY via `SolveStats::iterated_params` (participation
//!    counters), not wall-clock. Pre-slice, the whole component
//!    iterates: participation = every free parameter in it.
//! 3. **Cluster placement**: a rigid distance-triangle reachable only
//!    through cluster discovery (no vertex individually placeable)
//!    must take the planned path and land every dimension.
//! 4. **No-regression invariant** (spec §3.1 step 5): fallback
//!    honesty on under-/over-constrained and refuse-constraint
//!    sketches — identical verdicts, dense path taken.

#![allow(clippy::float_cmp)]
// Reason for `#![allow(clippy::expect_used)]` / `unwrap_used` /
// `panic` — test-only file: failing loudly at the fixture site is the
// desired failure mode; the workspace deny lints target production
// code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

mod common;

use common::{generate_plate, PlateSpec};
use geometry_engine::sketch2d::constraints::{
    Constraint, ConstraintPriority, DimensionalConstraint, EntityRef,
};
use geometry_engine::sketch2d::sketch::{Sketch, SketchAnchor};
use geometry_engine::sketch2d::sketch_solver::{build_solver, SolveOptions};
use geometry_engine::sketch2d::{Point2d, Point2dId};

/// Chain size for the serial-cluster scaling RED. Deliberately larger
/// than the LARGE plate's 32-point chain so the O(p³)-vs-serial gap is
/// decisive rather than marginal.
const CHAIN_POINTS: usize = 64;

/// Wall-clock budget (milliseconds of `SketchSolveReport::solve_time_ms`)
/// for the 64-point (128-constraint, 128-DOF) dimension chain under
/// the DEV test profile.
///
/// Derivation (measured on this machine, dev profile, 2026-07-16, via
/// the `#[ignore]`d `measure_chain_reference_timings` harness below —
/// recorded in `.superpowers/sdd/sketch-dcm-slice3-report.md`): the
/// serial expectation — every point placed as its own tiny solve
/// against a pinned predecessor — measured **171.1 ms** summed over 64
/// steps (a CONSERVATIVE ceiling: each mini-sketch pays full bridge
/// overhead and 4 parameters where a plan step solves 2). Budget =
/// ~5× that (900 ms), the gate-co-load margin policy the Slice-2
/// budgets settled on after a 2× margin flaked in the full-workspace
/// gate. The pre-slice whole-component dense path measured **4150 ms**
/// on the same chain (35 Newton iterations, each paying a
/// 128-parameter Jacobian + normal-equation elimination) — 4.6× over
/// budget: a genuine RED on the pre-slice solver, not a regression pin.
const CHAIN_BUDGET_MS: f64 = 900.0;

/// Wall-clock-asserting tests take this lock so they never time a
/// solve while a sibling test saturates the CPU (same pattern and
/// reasoning as the Slice-2 binary). Poisoned-lock recovery keeps a
/// sibling's failed assertion from masking this test's own verdict.
static TIMING_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn timing_guard() -> std::sync::MutexGuard<'static, ()> {
    TIMING_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn fresh(name: &str) -> Sketch {
    Sketch::new(name.to_string(), SketchAnchor::xy())
}

fn required_dim(dc: DimensionalConstraint, entities: Vec<EntityRef>) -> Constraint {
    Constraint::new_dimensional(dc, entities, ConstraintPriority::Required)
}

fn dist(a: (f64, f64), b: (f64, f64)) -> f64 {
    ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt()
}

// ── Fixtures ────────────────────────────────────────────────────────

/// A point jittered off its target, plus the target.
fn jittered_point(sketch: &Sketch, target: (f64, f64), seed: usize) -> (Point2dId, f64, f64) {
    let id = sketch.add_point(Point2d::new(
        target.0 + common::jitter(seed),
        target.1 + common::jitter(seed + 1),
    ));
    (id, target.0, target.1)
}

/// Anchor a point at its target with X + Y Required dimensions.
fn anchor(sketch: &Sketch, id: Point2dId, x: f64, y: f64) {
    sketch.add_constraint(required_dim(
        DimensionalConstraint::XCoordinate(x),
        vec![EntityRef::Point(id)],
    ));
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(y),
        vec![EntityRef::Point(id)],
    ));
}

/// Root + two dimensioned branches hanging off it — ONE connected
/// component whose DR-plan is a chain of single-entity extensions.
/// Returns (sketch, branch-A tip, all point ids with targets).
struct BranchFixture {
    sketch: Sketch,
    tip: Point2dId,
    tip_target: (f64, f64),
    points: Vec<(Point2dId, f64, f64)>,
    /// Total free parameters (2 per point).
    free_params: usize,
}

fn branch_fixture() -> BranchFixture {
    let sketch = fresh("dcm_slice3_branches");
    let mut points = Vec::new();

    let (root, rx, ry) = jittered_point(&sketch, (0.0, 0.0), 100);
    anchor(&sketch, root, rx, ry);
    points.push((root, rx, ry));

    // Branch A along y = 0, branch B along y = 6 — each point pinned
    // by Distance-to-previous + YCoordinate (a dimension chain).
    let grow = |prev: Point2dId,
                prev_t: (f64, f64),
                target: (f64, f64),
                seed: usize,
                points: &mut Vec<(Point2dId, f64, f64)>|
     -> (Point2dId, (f64, f64)) {
        let (id, tx, ty) = jittered_point(&sketch, target, seed);
        sketch.add_constraint(required_dim(
            DimensionalConstraint::Distance(dist(prev_t, target)),
            vec![EntityRef::Point(prev), EntityRef::Point(id)],
        ));
        sketch.add_constraint(required_dim(
            DimensionalConstraint::YCoordinate(ty),
            vec![EntityRef::Point(id)],
        ));
        points.push((id, tx, ty));
        (id, (tx, ty))
    };

    let (a1, a1t) = grow(root, (rx, ry), (10.0, 0.0), 110, &mut points);
    let (a2, a2t) = grow(a1, a1t, (20.0, 0.0), 120, &mut points);
    let (b1, b1t) = grow(root, (rx, ry), (8.0, 6.0), 130, &mut points);
    let (_b2, _b2t) = grow(b1, b1t, (13.0, 6.0), 140, &mut points);

    let free_params = points.len() * 2;
    BranchFixture {
        sketch,
        tip: a2,
        tip_target: a2t,
        points,
        free_params,
    }
}

/// Two anchors + a rigid distance triangle reachable ONLY through
/// cluster discovery (each vertex carries a single link to placed
/// geometry, so no vertex is individually placeable) + a pendant
/// dimension chain hanging off one triangle vertex.
struct TriangleFixture {
    sketch: Sketch,
    points: Vec<(Point2dId, f64, f64)>,
    /// (a, b, target distance) pairs to verify after the solve.
    distances: Vec<(Point2dId, Point2dId, f64)>,
}

fn triangle_pendant_fixture() -> TriangleFixture {
    let sketch = fresh("dcm_slice3_triangle");
    let mut points = Vec::new();
    let mut distances = Vec::new();

    let (g0, g0x, g0y) = jittered_point(&sketch, (0.0, 0.0), 200);
    anchor(&sketch, g0, g0x, g0y);
    points.push((g0, g0x, g0y));
    let (g1, g1x, g1y) = jittered_point(&sketch, (60.0, 0.0), 210);
    anchor(&sketch, g1, g1x, g1y);
    points.push((g1, g1x, g1y));

    let t0t = (20.0, 10.0);
    let t1t = (40.0, 25.0);
    let t2t = (15.0, 35.0);
    let (t0, ..) = jittered_point(&sketch, t0t, 220);
    let (t1, ..) = jittered_point(&sketch, t1t, 230);
    let (t2, ..) = jittered_point(&sketch, t2t, 240);
    points.push((t0, t0t.0, t0t.1));
    points.push((t1, t1t.0, t1t.1));
    points.push((t2, t2t.0, t2t.1));

    let mut add_distance = |a: Point2dId, at: (f64, f64), b: Point2dId, bt: (f64, f64)| {
        let d = dist(at, bt);
        sketch.add_constraint(required_dim(
            DimensionalConstraint::Distance(d),
            vec![EntityRef::Point(a), EntityRef::Point(b)],
        ));
        distances.push((a, b, d));
    };
    // Triangle shape (internal).
    add_distance(t0, t0t, t1, t1t);
    add_distance(t1, t1t, t2, t2t);
    add_distance(t0, t0t, t2, t2t);
    // Boundary: one link per vertex — no vertex individually
    // placeable — plus a frame Y closing the 3 placement DOF.
    add_distance(g0, (g0x, g0y), t0, t0t);
    add_distance(g1, (g1x, g1y), t1, t1t);
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(t2t.1),
        vec![EntityRef::Point(t2)],
    ));

    // Pendant chain off t2.
    let q1t = (t2t.0 + 6.0, t2t.1);
    let q2t = (q1t.0 + 6.0, q1t.1);
    let (q1, ..) = jittered_point(&sketch, q1t, 250);
    let (q2, ..) = jittered_point(&sketch, q2t, 260);
    points.push((q1, q1t.0, q1t.1));
    points.push((q2, q2t.0, q2t.1));
    add_distance(t2, t2t, q1, q1t);
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(q1t.1),
        vec![EntityRef::Point(q1)],
    ));
    add_distance(q1, q1t, q2, q2t);
    sketch.add_constraint(required_dim(
        DimensionalConstraint::YCoordinate(q2t.1),
        vec![EntityRef::Point(q2)],
    ));

    TriangleFixture {
        sketch,
        points,
        distances,
    }
}

fn assert_points_at_targets(sketch: &Sketch, points: &[(Point2dId, f64, f64)], tol: f64) {
    for (id, x, y) in points {
        let p = sketch.get_point(id).expect("point present");
        assert!(
            dist((p.x, p.y), (*x, *y)) < tol,
            "point missed its dimensioned target: ({}, {}) vs ({x}, {y})",
            p.x,
            p.y
        );
    }
}

// ── RED 1: serial-cluster scaling on the dimension chain ───────────

#[test]
fn red_dimension_chain_solves_within_serial_cluster_budget() {
    let _serial = timing_guard();
    let sketch = fresh("dcm_slice3_chain");
    let chain = common::add_chain(&sketch, CHAIN_POINTS, 0);
    let report = sketch.solve_constraints().expect("solve");

    assert!(
        report.is_fully_constrained(),
        "chain must converge fully constrained, got {:?}",
        report.status
    );
    for (id, x, y) in &chain {
        let p = sketch.get_point(id).expect("chain point");
        assert!(
            dist((p.x, p.y), (*x, *y)) < 1e-6,
            "chain point missed target: ({}, {}) vs ({x}, {y})",
            p.x,
            p.y
        );
    }
    assert!(
        report.solve_time_ms < CHAIN_BUDGET_MS,
        "serial-cluster scaling: {CHAIN_POINTS}-point dimension chain took \
         {:.1} ms, budget {CHAIN_BUDGET_MS} ms (~5× the measured serial \
         per-cluster expectation — the whole-component dense path pays \
         O(p³) per Newton iteration for a chain that is a sequence of \
         2-DOF rigid placements)",
        report.solve_time_ms
    );
}

// ── RED 2: drag re-solve scoping (structural, not wall-clock) ───────

#[test]
fn red_drag_resolves_only_the_dragged_cluster_chain() {
    let fixture = branch_fixture();
    let setup = fixture.sketch.solve_constraints().expect("setup solve");
    assert!(setup.is_fully_constrained(), "{:?}", setup.status);
    assert_points_at_targets(&fixture.sketch, &fixture.points, 1e-6);

    // Persist the drag pulls (identical rows to what solve_drag
    // synthesises — Low-priority X/Y coordinate pulls) so the drag
    // solver is reachable through `build_solver` and its
    // participation stats are readable.
    let target = (fixture.tip_target.0 + 3.0, fixture.tip_target.1 - 2.0);
    for dc in [
        DimensionalConstraint::XCoordinate(target.0),
        DimensionalConstraint::YCoordinate(target.1),
    ] {
        fixture.sketch.add_constraint(Constraint::new_dimensional(
            dc,
            vec![EntityRef::Point(fixture.tip)],
            ConstraintPriority::Low,
        ));
    }

    let mut solver = build_solver(&fixture.sketch, SolveOptions::for_drag()).expect("drag solver");
    let result = solver.solve();
    let stats = solver.last_stats();

    // The Required dimensions must hold the dragged tip at its
    // dimensioned position (drag semantics unchanged).
    match result
        .entity_updates
        .get(&EntityRef::Point(fixture.tip))
        .expect("tip update present")
    {
        geometry_engine::sketch2d::constraint_solver::EntityUpdate::Point(p) => {
            assert!(
                dist((p.x, p.y), fixture.tip_target) < 1e-3,
                "Required dims must win over the Low drag pull: ({}, {})",
                p.x,
                p.y
            );
        }
        other => panic!("unexpected update kind {other:?}"),
    }

    // THE structural scoping assertion: only the dragged point's
    // cluster (2 parameters) may take Newton iterations. Pre-slice
    // the whole component iterates — participation equals every free
    // parameter in the sketch.
    assert!(
        stats.iterated_params <= 2,
        "drag must re-solve only the dragged cluster chain: \
         {} of {} free parameters took Newton iterations (runs: {}) — \
         the whole component is re-solving",
        stats.iterated_params,
        fixture.free_params,
        stats.newton_runs_iterated
    );
    assert!(
        stats.iterated_params > 0,
        "the dragged cluster itself must iterate (the pull fights its dims)"
    );
}

// ── RED 3: rigid-cluster placement takes the planned path ───────────

#[test]
fn red_triangle_cluster_with_pendant_chain_takes_the_planned_path() {
    let fixture = triangle_pendant_fixture();
    let mut solver = build_solver(&fixture.sketch, SolveOptions::default()).expect("solver");
    let result = solver.solve();
    let stats = solver.last_stats();

    assert!(
        matches!(
            result.status,
            geometry_engine::sketch2d::constraint_solver::SolverStatus::Converged { .. }
        ),
        "triangle+pendant must converge: {:?}",
        result.status
    );
    // Structural: the single component must be solved via a complete
    // DR-plan (whose cluster step the dr_plan unit tests pin for this
    // exact topology), with zero dense fallbacks.
    assert_eq!(
        (
            stats.planned_components,
            stats.dense_components,
            stats.plan_fallbacks
        ),
        (1, 0, 0),
        "the triangle component must solve through the DR-plan \
         (cluster discovery + SE(2) placement), got {stats:?}"
    );

    // Geometry: land it in the sketch and verify every dimension.
    let report = fixture.sketch.solve_constraints().expect("bridge solve");
    assert!(report.is_fully_constrained(), "{:?}", report.status);
    assert_points_at_targets(&fixture.sketch, &fixture.points, 1e-6);
    for (a, b, d) in &fixture.distances {
        let pa = fixture.sketch.get_point(a).expect("a");
        let pb = fixture.sketch.get_point(b).expect("b");
        assert!(
            (dist((pa.x, pa.y), (pb.x, pb.y)) - d).abs() < 1e-6,
            "distance dimension missed: {} vs {d}",
            dist((pa.x, pa.y), (pb.x, pb.y))
        );
    }
}

// ── RED 4: the LARGE plate is fully planned ─────────────────────────

#[test]
fn red_large_plate_solves_fully_planned_with_no_dense_fallbacks() {
    let plate = generate_plate(&PlateSpec::LARGE);
    let mut solver = build_solver(&plate.sketch, SolveOptions::default()).expect("solver");
    let result = solver.solve();
    let stats = solver.last_stats();

    assert!(
        matches!(
            result.status,
            geometry_engine::sketch2d::constraint_solver::SolverStatus::Converged { .. }
        ),
        "LARGE plate must converge: {:?}",
        result.status
    );
    assert_eq!(
        (
            stats.planned_components,
            stats.dense_components,
            stats.plan_fallbacks
        ),
        (PlateSpec::LARGE.expected_components(), 0, 0),
        "every plate component (outline, chain, {} holes) must take \
         the planned path, got {stats:?}",
        PlateSpec::LARGE.holes
    );
}

// ── No-regression pins (green on BOTH paths) ────────────────────────

#[test]
fn planned_and_dense_paths_agree_on_the_large_plate() {
    use geometry_engine::sketch2d::constraint_solver::EntityUpdate;

    let plate = generate_plate(&PlateSpec::LARGE);
    let mut dense = build_solver(&plate.sketch, SolveOptions::default()).expect("dense");
    dense.set_dr_plan_enabled(false);
    let mut planned = build_solver(&plate.sketch, SolveOptions::default()).expect("planned");
    assert!(planned.dr_plan_enabled(), "default must be ON");

    let dense_result = dense.solve();
    let planned_result = planned.solve();

    assert!(
        matches!(
            (dense_result.status, planned_result.status),
            (
                geometry_engine::sketch2d::constraint_solver::SolverStatus::Converged { .. },
                geometry_engine::sketch2d::constraint_solver::SolverStatus::Converged { .. },
            )
        ),
        "status kinds must match: dense {:?} vs planned {:?}",
        dense_result.status,
        planned_result.status
    );
    assert!(dense_result.violations.is_empty());
    assert!(planned_result.violations.is_empty());

    assert_eq!(
        dense_result.entity_updates.len(),
        planned_result.entity_updates.len()
    );
    for (entity, dense_update) in &dense_result.entity_updates {
        let planned_update = planned_result
            .entity_updates
            .get(entity)
            .expect("entity present in both result sets");
        match (dense_update, planned_update) {
            (EntityUpdate::Point(a), EntityUpdate::Point(b)) => {
                assert!(
                    (a.x - b.x).abs() < 1e-8 && (a.y - b.y).abs() < 1e-8,
                    "point diverged between paths: {a:?} vs {b:?}"
                );
            }
            (EntityUpdate::Circle(ca, ra), EntityUpdate::Circle(cb, rb)) => {
                assert!(
                    (ca.x - cb.x).abs() < 1e-8
                        && (ca.y - cb.y).abs() < 1e-8
                        && (ra - rb).abs() < 1e-8,
                    "circle diverged between paths: ({ca:?}, {ra}) vs ({cb:?}, {rb})"
                );
            }
            (a, b) => panic!("update kind mismatch for {entity:?}: {a:?} vs {b:?}"),
        }
    }
}

#[test]
fn under_constrained_sketch_falls_back_to_dense_with_identical_verdict() {
    // A free point tied by one distance: 2 free DOFs remain. The
    // planner must refuse (incomplete), the dense core must run, and
    // the verdict must be the pre-slice one.
    let sketch = fresh("dcm_slice3_under");
    let p0 = sketch.add_point(Point2d::new(0.0, 0.0));
    let p1 = sketch.add_point(Point2d::new(3.0, 0.2));
    anchor(&sketch, p0, 0.0, 0.0);
    sketch.add_constraint(required_dim(
        DimensionalConstraint::Distance(5.0),
        vec![EntityRef::Point(p0), EntityRef::Point(p1)],
    ));

    let mut solver = build_solver(&sketch, SolveOptions::default()).expect("solver");
    let result = solver.solve();
    let stats = solver.last_stats();

    assert_eq!(
        result.status,
        geometry_engine::sketch2d::constraint_solver::SolverStatus::UnderConstrained {
            degrees_of_freedom: 1
        },
        "global DOF verdict unchanged"
    );
    assert!(result.violations.is_empty(), "{:?}", result.violations);
    assert_eq!(
        stats.planned_components, 0,
        "an under-constrained component must NOT be planned: {stats:?}"
    );
    assert!(stats.dense_components >= 1, "{stats:?}");
}

#[test]
fn refuse_constraint_component_falls_back_and_surfaces_the_violation() {
    // The #19 honest-refuse contract survives planning: a component
    // carrying a recognised-but-unenforced constraint refuses the
    // plan, solves dense, and surfaces the irreducible violation.
    let sketch = fresh("dcm_slice3_refuse");
    let q1 = sketch.add_point(Point2d::new(0.0, 0.0));
    let q2 = sketch.add_point(Point2d::new(5.0, 0.0));
    anchor(&sketch, q1, 0.0, 0.0);
    anchor(&sketch, q2, 5.0, 0.0);
    let refuse_id = sketch.add_constraint(required_dim(
        DimensionalConstraint::MinDistance(2.0),
        vec![EntityRef::Point(q1), EntityRef::Point(q2)],
    ));

    let mut solver = build_solver(&sketch, SolveOptions::default()).expect("solver");
    let result = solver.solve();
    let stats = solver.last_stats();

    assert!(
        result.violations.iter().any(|(id, _)| *id == refuse_id),
        "irreducible refuse residual must surface: {:?}",
        result.violations
    );
    assert_eq!(
        stats.planned_components, 0,
        "a refuse-carrying component must NOT be planned: {stats:?}"
    );
}

#[test]
fn conflicting_dimensions_fall_back_with_identical_overconstrained_verdict() {
    let build = || {
        let sketch = fresh("dcm_slice3_conflict");
        let p0 = sketch.add_point(Point2d::new(1.0, 1.0));
        anchor(&sketch, p0, 1.0, 1.0);
        // A third, conflicting X pin: structurally over-constrained.
        sketch.add_constraint(required_dim(
            DimensionalConstraint::XCoordinate(4.0),
            vec![EntityRef::Point(p0)],
        ));
        sketch
    };

    let mut dense = build_solver(&build(), SolveOptions::default()).expect("dense");
    dense.set_dr_plan_enabled(false);
    let mut planned = build_solver(&build(), SolveOptions::default()).expect("planned");

    let dense_result = dense.solve();
    let planned_result = planned.solve();
    assert_eq!(
        dense_result.status, planned_result.status,
        "over-constrained verdict must be path-independent"
    );
    assert_eq!(
        planned.last_stats().planned_components,
        0,
        "conflicting trio must refuse the plan (whole-or-nothing \
         consumption): {:?}",
        planned.last_stats()
    );
}

#[test]
fn repeated_planned_solves_are_bitwise_stable() {
    let sketch = fresh("dcm_slice3_stable");
    let chain = common::add_chain(&sketch, 8, 0);
    let first = sketch.solve_constraints().expect("first solve");
    assert!(first.is_fully_constrained(), "{:?}", first.status);

    let snapshot: Vec<(f64, f64)> = chain
        .iter()
        .map(|(id, _, _)| {
            let p = sketch.get_point(id).expect("point");
            (p.x, p.y)
        })
        .collect();
    let second = sketch.solve_constraints().expect("second solve");
    assert!(second.is_fully_constrained(), "{:?}", second.status);
    for ((id, _, _), (x, y)) in chain.iter().zip(snapshot) {
        let p = sketch.get_point(id).expect("point");
        assert_eq!(
            (p.x, p.y),
            (x, y),
            "re-solving a converged planned sketch must be a bitwise no-op"
        );
    }
}

#[test]
fn slice1_shared_endpoint_drag_semantics_hold_on_the_planned_path() {
    // The Slice-1 contract: dragging a shared endpoint moves both
    // incident entities. Under-constrained (drag actually moves
    // geometry) ⇒ the planner refuses and the dense path serves the
    // drag — semantics must be byte-compatible either way.
    use geometry_engine::sketch2d::sketch_solver::DragTarget;

    let sketch = fresh("dcm_slice3_slice1_drag");
    let a = sketch.add_point(Point2d::new(0.0, 0.0));
    let b = sketch.add_point(Point2d::new(10.0, 0.0));
    let line = sketch.add_line(a, b).expect("line");
    let _arc = sketch.add_arc(a, b, 7.0, true, false).expect("arc");

    let report = sketch
        .solve_drag(
            EntityRef::Point(b),
            DragTarget::Point(Point2d::new(12.0, 1.0)),
        )
        .expect("drag");
    assert!(
        report.violations.is_empty(),
        "unconstrained drag must satisfy its pulls: {:?}",
        report.violations
    );
    let pb = sketch.get_point(&b).expect("b");
    assert!(
        dist((pb.x, pb.y), (12.0, 1.0)) < 1e-3,
        "free point must land on the cursor: ({}, {})",
        pb.x,
        pb.y
    );
    // The line endpoint follows the shared point exactly (Slice-1
    // shared-variable semantics).
    let entry = sketch.lines().get(&line).expect("line present");
    let line_end = match entry.value().geometry {
        geometry_engine::sketch2d::line2d::LineGeometry::Segment(ref seg) => seg.end,
        ref other => panic!("expected segment, got {other:?}"),
    };
    assert!(
        dist((line_end.x, line_end.y), (pb.x, pb.y)) < 1e-9,
        "line endpoint must follow the shared dragged point"
    );
}

// ── Measurement harness (documents the budget derivation) ──────────

/// Not part of the gate — run explicitly with `--ignored --nocapture`
/// to reproduce the numbers behind `CHAIN_BUDGET_MS` and the
/// before/after table in the Slice-3 report. Records:
/// (a) the 64-point chain through the DEFAULT solver path,
/// (b) the same chain with the DR-plan disabled (the pre-slice
///     whole-component dense path),
/// (c) the serial expectation — every chain point solved as its own
///     2-parameter sketch against a pinned predecessor.
#[test]
#[ignore = "measurement harness: budget derivation, not a gate test"]
fn measure_chain_reference_timings() {
    let _serial = timing_guard();

    // (a) default path.
    let sketch = fresh("dcm_slice3_measure_default");
    common::add_chain(&sketch, CHAIN_POINTS, 0);
    let mut solver = build_solver(&sketch, SolveOptions::default()).expect("solver");
    let result = solver.solve();
    println!(
        "chain({CHAIN_POINTS}) default path: solve_time_ms = {:.2}, status = {:?}, stats = {:?}",
        result.solve_time_ms,
        result.status,
        solver.last_stats()
    );

    // (b) DR-plan disabled (pre-slice dense component path).
    let sketch = fresh("dcm_slice3_measure_dense");
    common::add_chain(&sketch, CHAIN_POINTS, 0);
    let mut solver = build_solver(&sketch, SolveOptions::default()).expect("solver");
    solver.set_dr_plan_enabled(false);
    let result = solver.solve();
    println!(
        "chain({CHAIN_POINTS}) dense (plan disabled): solve_time_ms = {:.2}, status = {:?}",
        result.solve_time_ms, result.status
    );

    // (c) serial expectation: point i solved alone against a pinned
    // predecessor — the DR-plan's per-step cost model.
    let mut serial_ms = 0.0;
    for i in 0..CHAIN_POINTS {
        let sketch = fresh("dcm_slice3_measure_serial");
        let tx = common::CHAIN_X0 + common::CHAIN_PITCH * i as f64;
        if i == 0 {
            let (id, x, y) = jittered_point(&sketch, (tx, common::CHAIN_Y), 9000 + 2 * i);
            anchor(&sketch, id, x, y);
        } else {
            let prev_t = (tx - common::CHAIN_PITCH, common::CHAIN_Y);
            let (prev, px, py) = jittered_point(&sketch, prev_t, 8000 + 2 * i);
            anchor(&sketch, prev, px, py);
            let (id, _, ty) = jittered_point(&sketch, (tx, common::CHAIN_Y), 9000 + 2 * i);
            sketch.add_constraint(required_dim(
                DimensionalConstraint::Distance(common::CHAIN_PITCH),
                vec![EntityRef::Point(prev), EntityRef::Point(id)],
            ));
            sketch.add_constraint(required_dim(
                DimensionalConstraint::YCoordinate(ty),
                vec![EntityRef::Point(id)],
            ));
        }
        let report = sketch.solve_constraints().expect("serial step");
        serial_ms += report.solve_time_ms;
    }
    println!("chain({CHAIN_POINTS}) serial per-cluster expectation: {serial_ms:.2} ms");
}
