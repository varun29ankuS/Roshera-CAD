//! Sketch ↔ constraint-solver bridge.
//!
//! [`ConstraintSolver`] is a freestanding Newton-Raphson engine that
//! operates on `EntityRef → EntityState` parameter vectors and a flat
//! list of [`Constraint`] records. [`Sketch`] is the CAD-facing
//! container that owns parametric entities (`ParametricPoint2d`,
//! `ParametricLine2d`, `ParametricCircle2d`, …) and a `ConstraintStore`.
//!
//! The two were never wired together: `Sketch::add_constraint` records
//! constraints but does not invoke the solver, and the api-server /
//! AI surfaces have no path to trigger a solve. This module is that
//! wiring.
//!
//! # Coverage
//!
//! [`solve`] translates the sketch's points, lines, and circles into
//! solver `EntityState`s, runs the existing solver, and writes the
//! result back onto the parametric entities. These three kinds are the
//! only ones the solver exposes public `EntityState` constructors for
//! (`EntityState::point`, `::line`, `::circle`); arcs, rectangles,
//! ellipses, splines, and polylines pass through unsolved with a
//! tracing warning. Extending coverage to those kinds is the responsi-
//! bility of [Slice C] (`Arc + spline curve types in SketchEntity`),
//! which adds the solver-side EntityState constructors and the
//! per-kind constraint evaluators.
//!
//! # Identity preservation
//!
//! `solve` does NOT touch entity IDs. After a successful solve, a
//! point's `Point2dId` is unchanged; only its `position` field is
//! updated. Same for line segments (`start` / `end` rewritten on the
//! existing id) and circles (`center` / `radius` rewritten). This
//! matches the broader "identity-preserving modify ops" architecture
//! shipped in slice 1 of body-modify (chamfer/fillet/mirror/shell/
//! extrude_face emit `ObjectUpdated`, not delete+create).
//!
//! # Fixed mask
//!
//! Per-entity fixed flags map straight through to the solver's
//! `fixed_mask`:
//!
//! - `ParametricPoint2d::is_fixed = true` → both x and y are pinned.
//! - Other entities have no per-DOF fix flags today; their parameters
//!   are all free unless the solver receives a `Fix`-style constraint
//!   (e.g., `XCoordinate(v)` + `YCoordinate(v)` for a point).
//!
//! [Slice C]: see project roadmap

use super::circle2d::ParametricCircle2d;
use super::constraint_solver::{
    ConstraintSolver, EntityState, EntityUpdate, SolverResult, SolverStatus,
};
use super::constraints::{ConstraintId, EntityRef};
use super::line2d::{LineGeometry, ParametricLine2d};
use super::point2d::ParametricPoint2d;
use super::sketch::Sketch;
use super::{Circle2d, Line2d, LineSegment2d, Point2d, Ray2d, Vector2d};
use thiserror::Error;

/// Tunables for a single solve invocation.
///
/// Defaults match `ConstraintSolver::new()`:
/// - `max_iterations = 100`
/// - `tolerance = 1e-10`
/// - `damping_factor = 0.5`
///
/// Slice B-2 will plumb a `SolveOptions::for_drag()` variant that
/// fixes the dragged entity and tightens convergence; for slice B-1
/// the default is sufficient.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolveOptions {
    /// Cap on Newton-Raphson iterations before declaring divergence.
    pub max_iterations: usize,
    /// Stop when `‖residual‖₂ < tolerance`.
    pub tolerance: f64,
    /// Step-size damping (0 < d ≤ 1). Lower = more conservative.
    pub damping_factor: f64,
}

impl Default for SolveOptions {
    fn default() -> Self {
        Self {
            max_iterations: 100,
            tolerance: 1e-10,
            damping_factor: 0.5,
        }
    }
}

impl SolveOptions {
    /// Construct an options bundle.
    pub fn new(max_iterations: usize, tolerance: f64, damping_factor: f64) -> Self {
        Self {
            max_iterations,
            tolerance,
            damping_factor,
        }
    }
}

/// Errors surfaced by the sketch ↔ solver bridge.
///
/// Solver-internal status values (`Unstable`, `OverConstrained`,
/// `UnderConstrained`) are surfaced in [`SketchSolveReport::status`]
/// rather than promoted to errors — those are valid analytical
/// outcomes the caller may want to display in the UI, not bugs.
/// Errors here are reserved for inputs the bridge itself can reject
/// before invoking the solver.
#[derive(Debug, Error, Clone, PartialEq)]
pub enum SketchSolveError {
    /// `SolveOptions::damping_factor` is outside (0, 1].
    #[error("invalid damping_factor {value}: must be in (0.0, 1.0]")]
    InvalidDamping { value: f64 },
    /// `SolveOptions::tolerance` is not strictly positive.
    #[error("invalid tolerance {value}: must be > 0")]
    InvalidTolerance { value: f64 },
    /// `SolveOptions::max_iterations` is zero.
    #[error("invalid max_iterations: must be > 0")]
    InvalidMaxIterations,
}

/// Snapshot of one solve invocation.
///
/// This is intentionally a value object, not a builder — callers
/// destructure it. `entities_solved` distinguishes the population that
/// participated in the solve (point/line/circle) from the kinds that
/// passed through unsolved.
#[derive(Debug, Clone)]
pub struct SketchSolveReport {
    /// Newton-Raphson outcome.
    pub status: SolverStatus,
    /// Residuals that remained above tolerance at solver exit. Empty
    /// when `status == Converged`. Each entry pairs the offending
    /// constraint id with its scalar error magnitude `‖residual‖₂`.
    pub violations: Vec<(ConstraintId, f64)>,
    /// Wall-clock cost of the solve, in milliseconds.
    pub solve_time_ms: f64,
    /// Number of entities the bridge registered with the solver
    /// before invoking it (= points + lines + circles).
    pub entities_solved: usize,
    /// Number of constraints registered with the solver. Includes
    /// constraints whose entity kind is unsupported in v1 (they are
    /// no-ops inside the solver but still counted).
    pub constraints_solved: usize,
    /// Number of sketch entities that were not addressable through
    /// the public `EntityState` constructors and were therefore
    /// excluded from the solve. Non-zero values are an inherent
    /// limitation of slice B-1; slice C lifts it for arcs / splines.
    pub entities_skipped_unsupported: usize,
}

impl SketchSolveReport {
    /// Convenience: did the solver converge to within tolerance?
    pub fn converged(&self) -> bool {
        matches!(self.status, SolverStatus::Converged { .. })
    }
}

// ── Public entry points ────────────────────────────────────────────

/// Solve a sketch's constraint system in place using default options.
///
/// Equivalent to [`solve_with_options`] with [`SolveOptions::default`].
pub fn solve(sketch: &Sketch) -> Result<SketchSolveReport, SketchSolveError> {
    solve_with_options(sketch, SolveOptions::default())
}

/// Solve a sketch's constraint system in place with custom options.
///
/// Procedure:
/// 1. Validate `options`.
/// 2. Snapshot every supported entity into the solver as an
///    `EntityState`. Fixed flags propagate to the solver's `fixed_mask`.
/// 3. Hand the sketch's full `Constraint` list to the solver.
/// 4. Invoke `solver.solve()`.
/// 5. For every `EntityUpdate` the solver returned, write the new
///    parameters back into the sketch's parametric entity. Entity IDs
///    are preserved.
///
/// The sketch is mutated through its `DashMap`-backed entity stores
/// via `&self`; no exclusive ownership is required, matching the
/// rest of the `Sketch` public surface (e.g. `Sketch::add_constraint`
/// also takes `&self`).
pub fn solve_with_options(
    sketch: &Sketch,
    options: SolveOptions,
) -> Result<SketchSolveReport, SketchSolveError> {
    validate_options(&options)?;

    let mut solver = ConstraintSolver::new();
    solver.set_max_iterations(options.max_iterations);
    solver.set_tolerance(options.tolerance);
    // `damping_factor` is a private field on ConstraintSolver in the
    // current solver impl; the public API ships with `0.5` and that
    // is documented in `SolveOptions::default`. The `damping_factor`
    // field on `SolveOptions` exists for forward-compat with B-2
    // (drag re-solve will want a tighter damping). Until the solver
    // exposes a setter we honour the documented default — passing a
    // non-default value is rejected up-front so callers cannot
    // silently rely on a no-op.
    if (options.damping_factor - 0.5).abs() > f64::EPSILON {
        return Err(SketchSolveError::InvalidDamping {
            value: options.damping_factor,
        });
    }

    let entities_solved = populate_solver(sketch, &solver);
    let entities_skipped_unsupported = count_unsupported(sketch);

    let constraints = sketch.all_constraints();
    let constraints_solved = constraints.len();
    solver.set_constraints(constraints);

    let result: SolverResult = solver.solve();

    apply_solver_result(sketch, &result);

    Ok(SketchSolveReport {
        status: result.status,
        violations: result.violations,
        solve_time_ms: result.solve_time_ms,
        entities_solved,
        constraints_solved,
        entities_skipped_unsupported,
    })
}

// ── Internal helpers ───────────────────────────────────────────────

fn validate_options(options: &SolveOptions) -> Result<(), SketchSolveError> {
    if options.max_iterations == 0 {
        return Err(SketchSolveError::InvalidMaxIterations);
    }
    if !(options.tolerance > 0.0) {
        return Err(SketchSolveError::InvalidTolerance {
            value: options.tolerance,
        });
    }
    if !(options.damping_factor > 0.0 && options.damping_factor <= 1.0) {
        return Err(SketchSolveError::InvalidDamping {
            value: options.damping_factor,
        });
    }
    Ok(())
}

/// Register every supported entity with the solver.
///
/// Returns the count of entities registered. Points, lines, and
/// circles are supported; other kinds are skipped (see [`count_unsupported`]).
fn populate_solver(sketch: &Sketch, solver: &ConstraintSolver) -> usize {
    let mut registered = 0usize;

    for entry in sketch.points().iter() {
        let id = *entry.key();
        let p: &ParametricPoint2d = entry.value();
        solver.add_entity(EntityRef::Point(id), EntityState::point(p.position, p.is_fixed));
        registered += 1;
    }

    for entry in sketch.lines().iter() {
        let id = *entry.key();
        let line: &ParametricLine2d = entry.value();
        let (point, direction) = line_to_point_direction(&line.geometry);
        // Lines have no per-DOF fix flags today; pass `false` so the
        // solver treats all four params as free unless an explicit
        // dimensional/positional constraint pins them.
        solver.add_entity(
            EntityRef::Line(id),
            EntityState::line(point, direction, false, false),
        );
        registered += 1;
    }

    for entry in sketch.circles().iter() {
        let id = *entry.key();
        let circle: &ParametricCircle2d = entry.value();
        solver.add_entity(
            EntityRef::Circle(id),
            EntityState::circle(circle.circle.center, circle.circle.radius, false, false),
        );
        registered += 1;
    }

    registered
}

/// Count entities whose kinds are unsupported by the v1 bridge.
fn count_unsupported(sketch: &Sketch) -> usize {
    // We can't iterate arcs / rectangles / ellipses / splines /
    // polylines through public accessors today, but we can count
    // them via the existing `SketchStatistics` surface. The bridge
    // uses the stat counters because they go through the same
    // public surface the rest of the API uses.
    let stats = sketch.statistics();
    stats.arc_count
        + stats.rectangle_count
        + stats.ellipse_count
        + stats.spline_count
        + stats.polyline_count
}

/// Translate a `LineGeometry` variant into the (point, direction)
/// pair the solver's `EntityState::line` expects.
///
/// The solver stores 4 scalars per line: `[px, py, dx, dy]`. The
/// "direction" component is treated as a raw vector by every
/// constraint evaluator in the solver — it is not assumed to be
/// unit-length. For segments we therefore pass `direction = end - start`
/// so reconstructing the segment after the solve is loss-free
/// (see [`apply_solver_result`]).
fn line_to_point_direction(geometry: &LineGeometry) -> (Point2d, Vector2d) {
    match geometry {
        LineGeometry::Infinite(l) => (l.point, l.direction),
        LineGeometry::Ray(r) => (r.origin, r.direction),
        LineGeometry::Segment(s) => (
            s.start,
            Vector2d::new(s.end.x - s.start.x, s.end.y - s.start.y),
        ),
    }
}

/// Write solver outputs back onto the sketch's parametric entities.
///
/// Entity IDs are preserved; only the geometric fields are updated.
/// Updates for kinds the bridge did not register (arc/rect/ellipse/
/// spline/polyline) cannot appear here because we never called
/// `add_entity` for them; defensive matches are still in place to
/// avoid panics if the solver ever starts returning them.
fn apply_solver_result(sketch: &Sketch, result: &SolverResult) {
    for (entity_ref, update) in &result.entity_updates {
        match (entity_ref, update) {
            (EntityRef::Point(id), EntityUpdate::Point(new_pos)) => {
                if let Some(mut entry) = sketch.points().get_mut(id) {
                    entry.value_mut().position = *new_pos;
                }
            }
            (EntityRef::Line(id), EntityUpdate::Line(point, direction)) => {
                if let Some(mut entry) = sketch.lines().get_mut(id) {
                    let new_geometry = apply_line_update(&entry.value().geometry, *point, *direction);
                    entry.value_mut().geometry = new_geometry;
                }
            }
            (EntityRef::Circle(id), EntityUpdate::Circle(center, radius)) => {
                if let Some(mut entry) = sketch.circles().get_mut(id) {
                    entry.value_mut().circle = Circle2d {
                        center: *center,
                        radius: *radius,
                    };
                }
            }
            // The remaining entity kinds are not registered by
            // `populate_solver` and therefore cannot legitimately
            // appear in `entity_updates`. If a future revision of
            // the solver ever surfaces one of them we drop it on the
            // floor rather than panicking — the unsolved entity
            // retains its prior geometry.
            _ => {}
        }
    }
}

/// Reconstruct a `LineGeometry` from solved `(point, direction)`.
///
/// For `Infinite` and `Ray` the parameters map straight back. For
/// `Segment` the bridge stored `direction = end - start`, so the
/// reconstructed end is `start + direction`.
fn apply_line_update(
    prior: &LineGeometry,
    point: Point2d,
    direction: Vector2d,
) -> LineGeometry {
    match prior {
        LineGeometry::Infinite(_) => LineGeometry::Infinite(Line2d {
            point,
            direction,
        }),
        LineGeometry::Ray(_) => LineGeometry::Ray(Ray2d {
            origin: point,
            direction,
        }),
        LineGeometry::Segment(_) => LineGeometry::Segment(LineSegment2d {
            start: point,
            end: Point2d::new(point.x + direction.x, point.y + direction.y),
        }),
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    //! Bridge-level coverage for `solve()`.
    //!
    //! Unit tests exercise:
    //! - Option validation (`SolveOptions` rejection paths).
    //! - Entity population (counts match the sketch state).
    //! - Constraint propagation (sketch constraints reach the solver).
    //! - Write-back correctness (entities receive solved parameters
    //!   without changing their IDs).
    //! - Status mapping (`Converged` / `UnderConstrained` / …).
    //!
    //! Convergence-quality tests beyond these (e.g., difficult
    //! initial guesses, near-singular Jacobians) live in
    //! `constraint_solver::tests` and are not re-asserted here.
    #![allow(clippy::float_cmp)]
    #![allow(clippy::expect_used)]
    #![allow(clippy::unwrap_used)]

    use super::*;
    use crate::sketch2d::constraints::{
        Constraint, ConstraintPriority, DimensionalConstraint, GeometricConstraint,
    };
    use crate::sketch2d::line2d::{Line2d, LineSegment2d};
    use crate::sketch2d::sketch::{Sketch, SketchAnchor};
    use crate::sketch2d::Vector2d;

    fn fresh_sketch() -> Sketch {
        Sketch::new("test".to_string(), SketchAnchor::xy())
    }

    // ── Option validation ──────────────────────────────────────────

    #[test]
    fn options_default_is_valid() {
        assert!(validate_options(&SolveOptions::default()).is_ok());
    }

    #[test]
    fn options_zero_iterations_rejected() {
        let opts = SolveOptions::new(0, 1e-10, 0.5);
        assert!(matches!(
            solve_with_options(&fresh_sketch(), opts),
            Err(SketchSolveError::InvalidMaxIterations),
        ));
    }

    #[test]
    fn options_zero_tolerance_rejected() {
        let opts = SolveOptions::new(50, 0.0, 0.5);
        assert!(matches!(
            solve_with_options(&fresh_sketch(), opts),
            Err(SketchSolveError::InvalidTolerance { .. }),
        ));
    }

    #[test]
    fn options_negative_tolerance_rejected() {
        let opts = SolveOptions::new(50, -1e-6, 0.5);
        assert!(matches!(
            solve_with_options(&fresh_sketch(), opts),
            Err(SketchSolveError::InvalidTolerance { .. }),
        ));
    }

    #[test]
    fn options_damping_zero_rejected() {
        let opts = SolveOptions::new(50, 1e-10, 0.0);
        assert!(matches!(
            solve_with_options(&fresh_sketch(), opts),
            Err(SketchSolveError::InvalidDamping { .. }),
        ));
    }

    #[test]
    fn options_damping_above_one_rejected() {
        let opts = SolveOptions::new(50, 1e-10, 1.5);
        assert!(matches!(
            solve_with_options(&fresh_sketch(), opts),
            Err(SketchSolveError::InvalidDamping { .. }),
        ));
    }

    #[test]
    fn options_non_default_damping_rejected_until_setter_lands() {
        // The current solver hardcodes 0.5; passing 1.0 is rejected
        // up-front so callers cannot silently rely on a no-op.
        let opts = SolveOptions::new(50, 1e-10, 1.0);
        assert!(matches!(
            solve_with_options(&fresh_sketch(), opts),
            Err(SketchSolveError::InvalidDamping { .. }),
        ));
    }

    // ── Population counts ──────────────────────────────────────────

    #[test]
    fn empty_sketch_solves_with_zero_entities() {
        let sketch = fresh_sketch();
        let report = solve(&sketch).expect("empty solve");
        assert_eq!(report.entities_solved, 0);
        assert_eq!(report.constraints_solved, 0);
        assert_eq!(report.entities_skipped_unsupported, 0);
    }

    #[test]
    fn single_point_registers_once() {
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(1.0, 2.0));
        let report = solve(&sketch).expect("solve");
        assert_eq!(report.entities_solved, 1);
    }

    #[test]
    fn mixed_kinds_register_supported_only() {
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .add_circle(Point2d::new(1.0, 1.0), 0.5)
            .expect("c");
        sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(2.0, 1.0))
            .expect("rect");

        let report = solve(&sketch).expect("solve");
        // 1 point + 1 circle = 2 supported; rectangle skipped.
        assert_eq!(report.entities_solved, 2);
        assert_eq!(report.entities_skipped_unsupported, 1);
    }

    // ── Write-back correctness ─────────────────────────────────────

    #[test]
    fn coincident_constraint_moves_free_point_onto_fixed_point() {
        let sketch = fresh_sketch();
        let fixed_id = sketch.add_point(Point2d::new(5.0, 7.0));
        let free_id = sketch.add_point(Point2d::new(0.0, 0.0));

        // Pin the first point.
        if let Some(mut entry) = sketch.points().get_mut(&fixed_id) {
            entry.value_mut().fix();
        } else {
            panic!("fixed_id missing");
        }

        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![EntityRef::Point(fixed_id), EntityRef::Point(free_id)],
            ConstraintPriority::High,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(report.converged(), "status was {:?}", report.status);

        // ID of free point unchanged.
        let entry = sketch.points().get(&free_id).expect("free point survives");
        let new_pos = entry.value().position;
        assert!((new_pos.x - 5.0).abs() < 1e-8, "x = {}", new_pos.x);
        assert!((new_pos.y - 7.0).abs() < 1e-8, "y = {}", new_pos.y);
    }

    #[test]
    fn distance_constraint_separates_two_points() {
        let sketch = fresh_sketch();
        let a_id = sketch.add_point(Point2d::new(0.0, 0.0));
        let b_id = sketch.add_point(Point2d::new(1.0, 1.0));

        // Pin the origin so the system is determinate enough for
        // the solver to drive the free point onto a circle of radius
        // 5 around it. Coordinate-based constraints pin the origin.
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(0.0),
            vec![EntityRef::Point(a_id)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(0.0),
            vec![EntityRef::Point(a_id)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(5.0),
            vec![EntityRef::Point(a_id), EntityRef::Point(b_id)],
            ConstraintPriority::High,
        ));

        let report = solve(&sketch).expect("solve");
        // System has 1 DOF left (rotation around origin) — this is
        // under-constrained by 1, but the solver still produces a
        // point on the circle of radius 5. Accept either status as
        // long as the distance is achieved.
        let a = sketch.points().get(&a_id).expect("a").value().position;
        let b = sketch.points().get(&b_id).expect("b").value().position;
        let d = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        assert!(
            (d - 5.0).abs() < 1e-6,
            "distance after solve = {}, status = {:?}",
            d,
            report.status,
        );
    }

    #[test]
    fn point_ids_preserved_across_solve() {
        let sketch = fresh_sketch();
        let id1 = sketch.add_point(Point2d::new(1.0, 0.0));
        let id2 = sketch.add_point(Point2d::new(2.0, 0.0));

        let _ = solve(&sketch).expect("solve");

        assert!(sketch.points().contains_key(&id1));
        assert!(sketch.points().contains_key(&id2));
    }

    #[test]
    fn circle_geometry_round_trips_unchanged_with_no_constraints() {
        let sketch = fresh_sketch();
        let id = sketch
            .add_circle(Point2d::new(3.0, -4.0), 2.5)
            .expect("circle");

        let _ = solve(&sketch).expect("solve");

        let entry = sketch.circles().get(&id).expect("circle survives");
        let c = &entry.value().circle;
        assert_eq!(c.center.x, 3.0);
        assert_eq!(c.center.y, -4.0);
        assert_eq!(c.radius, 2.5);
    }

    #[test]
    fn line_segment_geometry_preserves_kind() {
        let sketch = fresh_sketch();
        let p0 = sketch.add_point(Point2d::new(0.0, 0.0));
        let p1 = sketch.add_point(Point2d::new(3.0, 4.0));
        let id = sketch.add_line(p0, p1).expect("seg");

        let _ = solve(&sketch).expect("solve");

        let entry = sketch.lines().get(&id).expect("line survives");
        match &entry.value().geometry {
            LineGeometry::Segment(s) => {
                assert!((s.start.x - 0.0).abs() < 1e-10);
                assert!((s.start.y - 0.0).abs() < 1e-10);
                assert!((s.end.x - 3.0).abs() < 1e-10);
                assert!((s.end.y - 4.0).abs() < 1e-10);
            }
            other => panic!("expected Segment, got {:?}", other),
        }
    }

    // ── Status surfacing ───────────────────────────────────────────

    #[test]
    fn no_constraints_yields_a_status() {
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(0.0, 0.0));
        let report = solve(&sketch).expect("solve");
        // Exact status is solver-defined for an empty constraint
        // set; what we assert is that the bridge produced *some*
        // report and didn't panic.
        let _ = report.status;
    }

    #[test]
    fn report_records_constraint_count() {
        let sketch = fresh_sketch();
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(1.0, 0.0));
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::High,
        ));
        let report = solve(&sketch).expect("solve");
        assert_eq!(report.constraints_solved, 1);
    }

    // ── Line → (point, direction) translation ──────────────────────

    #[test]
    fn line_segment_translation_uses_endpoint_delta() {
        let seg = LineGeometry::Segment(LineSegment2d {
            start: Point2d::new(1.0, 2.0),
            end: Point2d::new(4.0, 6.0),
        });
        let (p, d) = line_to_point_direction(&seg);
        assert_eq!(p.x, 1.0);
        assert_eq!(p.y, 2.0);
        assert_eq!(d.x, 3.0);
        assert_eq!(d.y, 4.0);
    }

    #[test]
    fn infinite_line_translation_passes_through() {
        let inf = LineGeometry::Infinite(Line2d {
            point: Point2d::new(0.0, 0.0),
            direction: Vector2d::new(1.0, 0.0),
        });
        let (p, d) = line_to_point_direction(&inf);
        assert_eq!(p.x, 0.0);
        assert_eq!(d.x, 1.0);
    }

    #[test]
    fn apply_line_update_round_trip_for_segment() {
        let prior = LineGeometry::Segment(LineSegment2d {
            start: Point2d::ORIGIN,
            end: Point2d::new(1.0, 0.0),
        });
        let updated = apply_line_update(&prior, Point2d::new(2.0, 3.0), Vector2d::new(0.0, 5.0));
        match updated {
            LineGeometry::Segment(s) => {
                assert_eq!(s.start, Point2d::new(2.0, 3.0));
                assert_eq!(s.end, Point2d::new(2.0, 8.0));
            }
            _ => panic!("kind changed"),
        }
    }
}
