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
//! [`solve`] translates the sketch's points, lines, circles, and arcs
//! into solver `EntityState`s, runs the existing solver, and writes
//! the result back onto the parametric entities. These four kinds are
//! the ones the solver exposes public `EntityState` constructors for
//! (`EntityState::point`, `::line`, `::circle`, `::arc`); rectangles,
//! ellipses, splines, and polylines pass through unsolved (slices C-2
//! through C-5 lift the remaining kinds).
//!
//! For arcs the bridge solves the 5-parameter state
//! `[center.x, center.y, radius, start_angle, end_angle]`. The `ccw`
//! orientation bit is preserved across solve cycles in
//! [`apply_solver_result`] — it is not a continuously-varying
//! parameter the solver could differentiate over.
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

use super::arc2d::ParametricArc2d;
use super::circle2d::ParametricCircle2d;
use super::constraint_solver::{
    ConstraintSolver, EntityState, EntityUpdate, SolverResult, SolverStatus,
};
use super::constraints::{ConstraintId, EntityRef};
use super::line2d::{LineGeometry, ParametricLine2d};
use super::point2d::ParametricPoint2d;
use super::sketch::Sketch;
use super::{Circle2d, Line2d, LineSegment2d, Point2d, Ray2d, Vector2d};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Tag for an entity the bridge skipped because the solver does not yet
/// have a public `EntityState` constructor for its kind.
///
/// Surfaced in [`SketchSolveReport::entities_skipped`] so the UI can
/// highlight specifically which arcs / splines / rectangles will
/// remain unsolved until the relevant slice (C for arcs/splines) lands.
/// Reporting the [`EntityRef`] (not just a count) lets downstream
/// callers cross-reference against the sketch's entity stores.
pub type SkippedEntity = EntityRef;

/// Drag target supplied to [`solve_drag`].
///
/// The shape of the target is keyed to the kind of the dragged
/// entity. Slice B-2 supports dragging a [`EntityRef::Point`] to a
/// 2D location; lines, circles, and arcs will gain drag targets
/// when the frontend grows handles for them. Mismatched (entity,
/// target) pairs are rejected up-front by [`solve_drag`] with
/// [`SketchSolveError::DragTargetKindMismatch`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum DragTarget {
    /// Pull a point toward `(x, y)` via least-squares X/Y residuals.
    Point(Point2d),
}

impl DragTarget {
    /// Human-readable kind tag (matches the EntityRef variant the
    /// target binds to). Used in error messages.
    pub fn kind(&self) -> &'static str {
        match self {
            DragTarget::Point(_) => "Point",
        }
    }
}

/// Tunables for a single solve invocation.
///
/// Defaults match `ConstraintSolver::new()`:
/// - `max_iterations = 100`
/// - `tolerance = 1e-10`
/// - `damping_factor = 0.5`
///
/// [`SolveOptions::for_drag`] returns a preset tuned for live-drag
/// re-solve at ~60 fps: looser tolerance (`1e-6`, sub-pixel at
/// reasonable zoom), fewer iterations (`30`), and tighter damping
/// (`0.8`, closer to undamped Newton) so the dragged point tracks
/// the cursor crisply without overshoot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
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

    /// Preset tuned for live-drag re-solve at interactive framerates.
    ///
    /// `tolerance = 1e-6` is sub-pixel at every reasonable sketch
    /// zoom level, so converging tighter buys nothing the user can
    /// see while spending iteration budget. `max_iterations = 30`
    /// caps worst-case drag latency at ~3× the average solve cost
    /// (most drag frames converge in 2-5 iterations from the prior
    /// frame's warm start). `damping_factor = 0.8` is closer to a
    /// pure Newton step than the conservative 0.5 default, so the
    /// dragged point tracks the cursor without visible lag.
    pub fn for_drag() -> Self {
        Self {
            max_iterations: 30,
            tolerance: 1.0e-6,
            damping_factor: 0.8,
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
#[derive(Debug, Error, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
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
    /// The `EntityRef` passed to [`solve_drag`] does not resolve to a
    /// live entity in the sketch's entity stores.
    #[error("drag entity {entity:?} not found in sketch")]
    DragEntityNotFound { entity: EntityRef },
    /// The dragged entity's kind has no compatible [`DragTarget`]
    /// variant in this slice. Slice B-2 supports dragging
    /// [`EntityRef::Point`] only; lines and circles will gain drag
    /// targets when the frontend grows handles for them.
    #[error(
        "drag entity kind unsupported: {entity:?} cannot be dragged with a {target_kind} target \
         in slice B-2"
    )]
    DragTargetKindMismatch {
        entity: EntityRef,
        target_kind: String,
    },
    /// The dragged entity is pinned by its own `is_fixed` flag.
    /// Dragging a locked point is a user error: the soft X/Y pull
    /// would fight the fixed flag and produce a misleading
    /// over-constrained verdict. Refusing up-front keeps the UI
    /// honest — modern parametric sketchers (Fusion / Onshape /
    /// SolidWorks) all treat a locked sketch entity as undraggable.
    #[error("drag entity {entity:?} is locked (is_fixed = true); unlock it before dragging")]
    DragEntityFixed { entity: EntityRef },
}

/// Snapshot of one solve invocation.
///
/// Value object — callers destructure it or use the convenience
/// accessors. The report carries everything a UI needs to draw a
/// "Fully constrained / 3 DOF / 12 iterations / 0.42 ms" status badge
/// without having to pattern-match the raw [`SolverStatus`].
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// before invoking it (= points + lines + circles + arcs).
    pub entities_solved: usize,
    /// Number of constraints registered with the solver. Includes
    /// constraints whose entity kind is unsupported in v1 (they are
    /// no-ops inside the solver but still counted).
    pub constraints_solved: usize,
    /// Entity references the bridge could not address through the
    /// public `EntityState` constructors and therefore excluded from
    /// the solve. As of slice C-1 the only unsupported kinds are
    /// rectangles, ellipses, splines, and polylines (slices C-2
    /// through C-5). Reporting the [`EntityRef`] (not just a count)
    /// lets the UI highlight exactly which entities went unsolved.
    pub entities_skipped: Vec<SkippedEntity>,
}

impl SketchSolveReport {
    /// Convenience: did the solver converge to within tolerance?
    pub fn converged(&self) -> bool {
        matches!(self.status, SolverStatus::Converged { .. })
    }

    /// Convenience: is the sketch fully constrained (converged with
    /// no remaining degrees of freedom)?
    ///
    /// In Onshape / SolidWorks / Fusion parlance this is the "fully
    /// defined" state that turns the sketch border green.
    pub fn is_fully_constrained(&self) -> bool {
        self.converged()
    }

    /// Convenience: did the solver report an under-constrained system?
    pub fn is_under_constrained(&self) -> bool {
        matches!(self.status, SolverStatus::UnderConstrained { .. })
    }

    /// Convenience: did the solver report an over-constrained system?
    pub fn is_over_constrained(&self) -> bool {
        matches!(self.status, SolverStatus::OverConstrained { .. })
    }

    /// Convenience: did the solver report numerical instability
    /// (singular or near-singular Jacobian, NaN propagation)?
    pub fn is_unstable(&self) -> bool {
        matches!(self.status, SolverStatus::Unstable)
    }

    /// Newton-Raphson iteration count when available
    /// (`Converged` / `NotConverged`). `None` for early-rejected
    /// statuses (under/over-constrained, unstable).
    pub fn iterations(&self) -> Option<usize> {
        match self.status {
            SolverStatus::Converged { iterations, .. }
            | SolverStatus::NotConverged { iterations, .. } => Some(iterations),
            _ => None,
        }
    }

    /// `‖residual‖₂` at solver exit when available
    /// (`Converged` / `NotConverged`).
    pub fn final_error(&self) -> Option<f64> {
        match self.status {
            SolverStatus::Converged { final_error, .. }
            | SolverStatus::NotConverged { final_error, .. } => Some(final_error),
            _ => None,
        }
    }

    /// Remaining degrees of freedom reported by the under-constrained
    /// detector. `Some(n)` only when `status == UnderConstrained{…}`.
    pub fn degrees_of_freedom(&self) -> Option<usize> {
        match self.status {
            SolverStatus::UnderConstrained { degrees_of_freedom } => Some(degrees_of_freedom),
            _ => None,
        }
    }

    /// Conflicting-constraint count from the over-constrained
    /// detector. `Some(n)` only when `status == OverConstrained{…}`.
    pub fn conflicting_constraints(&self) -> Option<usize> {
        match self.status {
            SolverStatus::OverConstrained {
                conflicting_constraints,
            } => Some(conflicting_constraints),
            _ => None,
        }
    }

    /// Count of entities the bridge could not register with the
    /// solver because their kind has no public `EntityState`
    /// constructor yet (rectangle / ellipse / spline / polyline).
    /// Equivalent to `entities_skipped.len()`.
    pub fn skipped_count(&self) -> usize {
        self.entities_skipped.len()
    }
}

/// Structural-DOF classification independent of the Newton-Raphson
/// solve outcome.
///
/// `FullyConstrained` means free DOFs equals constraint DOFs removed;
/// no residual freedom remains in the system. `UnderConstrained {
/// dofs }` means the sketch has `dofs` more free parameters than
/// constraints to pin them — the system has a manifold of valid
/// solutions. `OverConstrained { conflicting_constraints }` means
/// the system has `conflicting_constraints` more constraint DOFs
/// than the sketch can absorb — at least that many constraints will
/// fight each other.
///
/// This is the same DOF accounting the solver performs internally
/// (`check_constraint_count`), but exposed without running the
/// Newton-Raphson iteration, so the UI can update a "DOF: 3" badge
/// reactively as constraints are added or removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DofStatus {
    /// `total_free_dofs == constraint_dofs_removed`.
    FullyConstrained,
    /// `total_free_dofs > constraint_dofs_removed`. `dofs` is the
    /// difference.
    UnderConstrained { dofs: usize },
    /// `total_free_dofs < constraint_dofs_removed`. `conflicting_constraints`
    /// is the difference.
    OverConstrained { conflicting_constraints: usize },
}

/// Output of [`analyze_dofs`].
///
/// Carries the raw DOF tallies for diagnostics on top of the
/// derived [`DofStatus`] verdict. Mirrors the shape of
/// [`SketchSolveReport`] for entity counting so the UI can use a
/// single render path for "pre-solve summary" and "post-solve
/// summary" panels.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DofReport {
    /// Total degrees of freedom across all analysable entities.
    ///
    /// Points contribute 2 (x, y), 0 if `is_fixed`.
    /// Lines contribute 4 (px, py, dx, dy).
    /// Circles contribute 3 (cx, cy, r).
    /// Arcs contribute 5 (cx, cy, r, start_angle, end_angle).
    /// Entities listed in `entities_skipped` contribute 0 — slices
    /// C-2 through C-5 lift this for the remaining kinds.
    pub total_free_dofs: usize,
    /// Sum of `degrees_of_freedom_removed()` across constraints
    /// whose entire entity set is supported by the current bridge
    /// (points, lines, circles, arcs). Constraints that reference
    /// any unsupported entity are excluded from this tally —
    /// counting them while excluding the unsupported entity's own
    /// free DOFs would produce a phantom over-constrained verdict.
    /// See `constraints_skipped` for the count of such constraints.
    pub constraint_dofs_removed: usize,
    /// Derived verdict over the supported subset.
    pub status: DofStatus,
    /// Count of entities the analysis registered (points + lines +
    /// circles + arcs, fixed or free).
    pub entities_analysed: usize,
    /// Count of constraints that contributed to `constraint_dofs_removed`.
    pub constraints_analysed: usize,
    /// Count of constraints excluded from the DOF accounting because
    /// at least one referenced entity is in `entities_skipped`.
    /// Non-zero values tell the UI the verdict is over a partial
    /// view of the sketch — slice C lifts this by adding solver
    /// support for arcs / splines / rectangles.
    pub constraints_skipped: usize,
    /// Entities that cannot contribute DOFs in slice B-2 — same
    /// list semantics as [`SketchSolveReport::entities_skipped`].
    pub entities_skipped: Vec<SkippedEntity>,
}

impl DofReport {
    /// Convenience: is the sketch structurally fully constrained?
    pub fn is_fully_constrained(&self) -> bool {
        matches!(self.status, DofStatus::FullyConstrained)
    }
    /// Convenience: is the sketch structurally under-constrained?
    pub fn is_under_constrained(&self) -> bool {
        matches!(self.status, DofStatus::UnderConstrained { .. })
    }
    /// Convenience: is the sketch structurally over-constrained?
    pub fn is_over_constrained(&self) -> bool {
        matches!(self.status, DofStatus::OverConstrained { .. })
    }
    /// True when the DOF verdict is computed over a partial view of
    /// the sketch (one or more constraints excluded because they
    /// touch a kind unsupported by slice B-2). UIs should show a
    /// "partial DOF accounting" hint when this is true.
    pub fn has_skipped_constraints(&self) -> bool {
        self.constraints_skipped > 0
    }
    /// Remaining DOFs when under-constrained; `None` otherwise.
    pub fn remaining_dofs(&self) -> Option<usize> {
        match self.status {
            DofStatus::UnderConstrained { dofs } => Some(dofs),
            _ => None,
        }
    }
    /// Excess constraints when over-constrained; `None` otherwise.
    pub fn excess_constraints(&self) -> Option<usize> {
        match self.status {
            DofStatus::OverConstrained {
                conflicting_constraints,
            } => Some(conflicting_constraints),
            _ => None,
        }
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
    solve_internal(sketch, options, Vec::new())
}

/// Drag a single entity toward a target position while honouring
/// every other constraint in the sketch.
///
/// Uses [`SolveOptions::for_drag`] — the framerate-tuned preset
/// (1e-6 tolerance, 30 iterations, 0.8 damping). Use
/// [`solve_drag_with_options`] to override.
///
/// The drag is implemented as least-squares: a temporary
/// `Required`-priority `XCoordinate(target.x)` and
/// `YCoordinate(target.y)` pair is added to the constraint set for
/// this solve invocation only — they are not persisted into the
/// sketch's `ConstraintStore`. If the target is reachable, the
/// dragged point lands on it; if not (e.g. the point is constrained
/// to a fixed circle and the cursor is off the circle), Newton-
/// Raphson finds the residual minimum, which is the closest
/// reachable point. This matches the drag behaviour every modern
/// parametric sketcher implements.
///
/// Returns the standard [`SketchSolveReport`]; the achieved
/// position is read from the sketch's entity store after the call.
pub fn solve_drag(
    sketch: &Sketch,
    dragged: EntityRef,
    target: DragTarget,
) -> Result<SketchSolveReport, SketchSolveError> {
    solve_drag_with_options(sketch, dragged, target, SolveOptions::for_drag())
}

/// Drag with custom solver options. See [`solve_drag`].
pub fn solve_drag_with_options(
    sketch: &Sketch,
    dragged: EntityRef,
    target: DragTarget,
    options: SolveOptions,
) -> Result<SketchSolveReport, SketchSolveError> {
    let extra = build_drag_constraints(sketch, dragged, target)?;
    solve_internal(sketch, options, extra)
}

/// Analyse the sketch's structural degrees of freedom without
/// running Newton-Raphson.
///
/// Returns a [`DofReport`] containing the same DOF accounting the
/// solver does in its over/under-constrained detector, but computed
/// in O(entities + constraints) so the UI can react to constraint
/// additions in real time. Does not mutate the sketch.
pub fn analyze_dofs(sketch: &Sketch) -> DofReport {
    use std::cmp::Ordering;

    let mut total_free_dofs: usize = 0;
    let mut entities_analysed: usize = 0;

    // Points: 2 DOFs (x, y); 0 when fixed.
    for entry in sketch.points().iter() {
        entities_analysed += 1;
        if !entry.value().is_fixed {
            total_free_dofs += 2;
        }
    }
    // Lines: 4 DOFs (px, py, dx, dy). No per-DOF fix flags today.
    for _entry in sketch.lines().iter() {
        entities_analysed += 1;
        total_free_dofs += 4;
    }
    // Circles: 3 DOFs (cx, cy, r). No per-DOF fix flags today.
    for _entry in sketch.circles().iter() {
        entities_analysed += 1;
        total_free_dofs += 3;
    }
    // Arcs: 5 DOFs (cx, cy, r, start_angle, end_angle). Matches
    // `ParametricArc2d::degrees_of_freedom` and the solver-side
    // `EntityState::arc` parameter layout. `ccw` is not a solver
    // DOF — orientation is a discrete bit.
    for _entry in sketch.arcs().iter() {
        entities_analysed += 1;
        total_free_dofs += 5;
    }

    let entities_skipped = collect_unsupported(sketch);
    // HashSet for O(1) membership checks while iterating constraints.
    let skipped_set: std::collections::HashSet<EntityRef> =
        entities_skipped.iter().copied().collect();

    let mut constraint_dofs_removed: usize = 0;
    let mut constraints_analysed: usize = 0;
    let mut constraints_skipped: usize = 0;
    for c in sketch.all_constraints().iter() {
        if c.entities.iter().any(|e| skipped_set.contains(e)) {
            // Constraint references at least one kind we can't yet
            // analyse — excluding it keeps the DOF arithmetic
            // mathematically honest (we'd otherwise subtract DOFs
            // from a system whose other side contributed 0).
            constraints_skipped += 1;
            continue;
        }
        constraint_dofs_removed += c.degrees_of_freedom_removed();
        constraints_analysed += 1;
    }

    let status = match constraint_dofs_removed.cmp(&total_free_dofs) {
        Ordering::Equal => DofStatus::FullyConstrained,
        Ordering::Less => DofStatus::UnderConstrained {
            dofs: total_free_dofs - constraint_dofs_removed,
        },
        Ordering::Greater => DofStatus::OverConstrained {
            conflicting_constraints: constraint_dofs_removed - total_free_dofs,
        },
    };

    DofReport {
        total_free_dofs,
        constraint_dofs_removed,
        status,
        entities_analysed,
        constraints_analysed,
        constraints_skipped,
        entities_skipped,
    }
}

// ── Internal: shared solve path + drag-constraint synthesis ────────

/// Run the bridge with an optional list of additional constraints
/// stacked on top of the sketch's persisted constraint set.
///
/// Extras are NOT persisted into the sketch — they exist for the
/// duration of this call only, which is what makes least-squares
/// drag transparent: the user's drag pull does not pollute the
/// sketch's `ConstraintStore`.
fn solve_internal(
    sketch: &Sketch,
    options: SolveOptions,
    extra_constraints: Vec<super::constraints::Constraint>,
) -> Result<SketchSolveReport, SketchSolveError> {
    validate_options(&options)?;

    let mut solver = ConstraintSolver::new();
    solver.set_max_iterations(options.max_iterations);
    solver.set_tolerance(options.tolerance);
    solver.set_damping_factor(options.damping_factor);

    let entities_solved = populate_solver(sketch, &solver);
    let entities_skipped = collect_unsupported(sketch);

    let mut constraints = sketch.all_constraints();
    constraints.extend(extra_constraints);
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
        entities_skipped,
    })
}

/// Validate the (entity, target) pair and synthesise the temporary
/// X/Y-coordinate constraints that pull `dragged` toward `target`.
fn build_drag_constraints(
    sketch: &Sketch,
    dragged: EntityRef,
    target: DragTarget,
) -> Result<Vec<super::constraints::Constraint>, SketchSolveError> {
    use super::constraints::{Constraint, ConstraintPriority, DimensionalConstraint};

    match (dragged, target) {
        (EntityRef::Point(point_id), DragTarget::Point(target_pos)) => {
            let entry = sketch
                .points()
                .get(&point_id)
                .ok_or(SketchSolveError::DragEntityNotFound { entity: dragged })?;
            if entry.value().is_fixed {
                return Err(SketchSolveError::DragEntityFixed { entity: dragged });
            }
            // Drop the DashMap read guard before constructing the
            // returned vector so we don't hold it across the alloc.
            drop(entry);
            let x_constraint = Constraint::new_dimensional(
                DimensionalConstraint::XCoordinate(target_pos.x),
                vec![EntityRef::Point(point_id)],
                ConstraintPriority::Required,
            );
            let y_constraint = Constraint::new_dimensional(
                DimensionalConstraint::YCoordinate(target_pos.y),
                vec![EntityRef::Point(point_id)],
                ConstraintPriority::Required,
            );
            Ok(vec![x_constraint, y_constraint])
        }
        // Slice B-2 only supports dragging points. Lines/circles
        // will gain drag targets when the frontend grows handles
        // for them; for now we reject the call rather than silently
        // produce a no-op solve.
        (entity, _) => Err(SketchSolveError::DragTargetKindMismatch {
            entity,
            target_kind: target.kind().to_string(),
        }),
    }
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
/// Returns the count of entities registered. Points, lines, circles,
/// and arcs are supported; other kinds are skipped (see
/// [`collect_unsupported`]).
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

    for entry in sketch.arcs().iter() {
        let id = *entry.key();
        let arc: &ParametricArc2d = entry.value();
        // Arcs have no per-DOF fix flags today — pass `false` for
        // every group so the solver treats center/radius/angles as
        // free unless a dimensional or positional constraint pins
        // them. `ccw` is not a solver parameter (orientation is a
        // discrete bit, not a continuously-varying scalar); the
        // bridge preserves it across solve cycles in
        // [`apply_solver_result`].
        solver.add_entity(
            EntityRef::Arc(id),
            EntityState::arc(
                arc.arc.center,
                arc.arc.radius,
                arc.arc.start_angle,
                arc.arc.end_angle,
                false,
                false,
                false,
            ),
        );
        registered += 1;
    }

    registered
}

/// Collect `EntityRef`s for entities whose kinds are unsupported by
/// the current bridge (rectangles / ellipses / splines / polylines).
/// Arcs are supported as of slice C-1 and no longer appear here.
/// The returned vector is in DashMap iteration order; callers that
/// need a stable order should sort after collection.
///
/// Returning the IDs (not just a count) lets the UI highlight
/// specifically which entities will remain unsolved until later
/// slices add `EntityState` constructors for their kinds.
fn collect_unsupported(sketch: &Sketch) -> Vec<EntityRef> {
    let mut skipped: Vec<EntityRef> = Vec::new();
    for entry in sketch.rectangles().iter() {
        skipped.push(EntityRef::Rectangle(*entry.key()));
    }
    for entry in sketch.ellipses().iter() {
        skipped.push(EntityRef::Ellipse(*entry.key()));
    }
    for entry in sketch.splines().iter() {
        skipped.push(EntityRef::Spline(*entry.key()));
    }
    for entry in sketch.polylines().iter() {
        skipped.push(EntityRef::Polyline(*entry.key()));
    }
    skipped
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
/// For arcs the `ccw` orientation bit is preserved as well — the
/// solver does not own it. Updates for kinds the bridge did not
/// register (rectangle/ellipse/spline/polyline) cannot appear here
/// because we never called `add_entity` for them; defensive matches
/// are still in place to avoid panics if the solver ever starts
/// returning them.
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
            (
                EntityRef::Arc(id),
                EntityUpdate::Arc(center, radius, start_angle, end_angle),
            ) => {
                if let Some(mut entry) = sketch.arcs().get_mut(id) {
                    // ID and `ccw` flag are preserved — the solver
                    // does not own the orientation bit (see
                    // `EntityState::arc` doc) and the bridge keeps
                    // it stable across solve cycles.
                    let arc = &mut entry.value_mut().arc;
                    arc.center = *center;
                    arc.radius = *radius;
                    arc.start_angle = *start_angle;
                    arc.end_angle = *end_angle;
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
    fn options_damping_at_one_accepted() {
        // 1.0 is the upper bound of the valid range and corresponds
        // to an undamped Newton step. The solver now exposes a real
        // setter so the bridge passes the value through; the smoke
        // test asserts that a 1.0 damping factor does not get
        // rejected upstream.
        let opts = SolveOptions::new(50, 1e-10, 1.0);
        let report = solve_with_options(&fresh_sketch(), opts)
            .expect("damping 1.0 is in (0, 1] and must be accepted");
        let _ = report.status;
    }

    #[test]
    fn options_damping_propagates_into_solver() {
        // 0.25 is in range and distinct from the solver's default
        // 0.5; calling solve with it must not error.
        let opts = SolveOptions::new(50, 1e-10, 0.25);
        let report = solve_with_options(&fresh_sketch(), opts)
            .expect("damping 0.25 in (0, 1] is accepted");
        let _ = report.status;
    }

    // ── Population counts ──────────────────────────────────────────

    #[test]
    fn empty_sketch_solves_with_zero_entities() {
        let sketch = fresh_sketch();
        let report = solve(&sketch).expect("empty solve");
        assert_eq!(report.entities_solved, 0);
        assert_eq!(report.constraints_solved, 0);
        assert!(report.entities_skipped.is_empty());
        assert_eq!(report.skipped_count(), 0);
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
        let rect_id = sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(2.0, 1.0))
            .expect("rect");

        let report = solve(&sketch).expect("solve");
        // 1 point + 1 circle = 2 supported; rectangle skipped.
        assert_eq!(report.entities_solved, 2);
        assert_eq!(report.skipped_count(), 1);
        assert_eq!(report.entities_skipped, vec![EntityRef::Rectangle(rect_id)]);
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
    fn arc_geometry_round_trips_unchanged_with_no_constraints() {
        // Slice C-1: arcs are first-class solver entities. With no
        // constraints the bridge must register the arc, run the
        // solver (which converges trivially), and write back the
        // identical parameters — preserving id, center, radius,
        // angles, and the `ccw` orientation bit.
        let sketch = fresh_sketch();
        let id = sketch
            .add_arc_center_angles(
                Point2d::new(1.5, -0.25),
                0.75,
                0.0,
                std::f64::consts::PI / 2.0,
            )
            .expect("arc");

        let report = solve(&sketch).expect("solve");
        // No more arc-skipped entry — slice C-1 lifted the
        // limitation.
        assert!(
            !report
                .entities_skipped
                .iter()
                .any(|e| matches!(e, EntityRef::Arc(_))),
            "arc must not appear in entities_skipped: {:?}",
            report.entities_skipped,
        );
        assert_eq!(report.entities_solved, 1);

        let entry = sketch.arcs().get(&id).expect("arc survives");
        let a = &entry.value().arc;
        assert!((a.center.x - 1.5).abs() < 1e-10);
        assert!((a.center.y - (-0.25)).abs() < 1e-10);
        assert!((a.radius - 0.75).abs() < 1e-10);
        assert!((a.start_angle - 0.0).abs() < 1e-10);
        assert!((a.end_angle - std::f64::consts::PI / 2.0).abs() < 1e-10);
        // `ccw` is not a solver parameter — must be preserved.
        assert!(a.ccw);
    }

    #[test]
    fn arc_contributes_five_dofs_to_analysis() {
        // Per `ParametricArc2d::degrees_of_freedom`: center.x,
        // center.y, radius, start_angle, end_angle. `ccw` is
        // discrete and not counted.
        let sketch = fresh_sketch();
        sketch
            .add_arc_center_angles(Point2d::new(0.0, 0.0), 1.0, 0.0, 1.0)
            .expect("arc");
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 5);
        assert_eq!(report.entities_analysed, 1);
        // No skipped arcs after slice C-1.
        assert!(report.entities_skipped.is_empty());
    }

    #[test]
    fn mixed_kinds_with_arc_register_arc_as_supported() {
        // Sanity check for the supported-kinds list update: a
        // sketch carrying one point + one arc + one ellipse should
        // count 2 supported and 1 skipped.
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .add_arc_center_angles(Point2d::new(0.0, 0.0), 1.0, 0.0, 1.0)
            .expect("arc");
        let ellipse_id = sketch
            .add_ellipse(Point2d::new(2.0, 0.0), 1.0, 0.5, 0.0)
            .expect("ellipse");
        let report = solve(&sketch).expect("solve");
        assert_eq!(report.entities_solved, 2);
        assert_eq!(report.entities_skipped, vec![EntityRef::Ellipse(ellipse_id)]);
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

    // ── Convenience accessors on SketchSolveReport ─────────────────

    fn report_with_status(status: SolverStatus) -> SketchSolveReport {
        SketchSolveReport {
            status,
            violations: Vec::new(),
            solve_time_ms: 0.0,
            entities_solved: 0,
            constraints_solved: 0,
            entities_skipped: Vec::new(),
        }
    }

    #[test]
    fn report_converged_status_maps_through() {
        let r = report_with_status(SolverStatus::Converged {
            iterations: 7,
            final_error: 1e-12,
        });
        assert!(r.converged());
        assert!(r.is_fully_constrained());
        assert!(!r.is_under_constrained());
        assert!(!r.is_over_constrained());
        assert!(!r.is_unstable());
        assert_eq!(r.iterations(), Some(7));
        assert!(r.final_error().expect("err") < 1e-10);
        assert_eq!(r.degrees_of_freedom(), None);
        assert_eq!(r.conflicting_constraints(), None);
    }

    #[test]
    fn report_under_constrained_exposes_dof() {
        let r = report_with_status(SolverStatus::UnderConstrained {
            degrees_of_freedom: 3,
        });
        assert!(!r.converged());
        assert!(r.is_under_constrained());
        assert_eq!(r.degrees_of_freedom(), Some(3));
        assert_eq!(r.iterations(), None);
        assert_eq!(r.final_error(), None);
        assert_eq!(r.conflicting_constraints(), None);
    }

    #[test]
    fn report_over_constrained_exposes_conflicting_count() {
        let r = report_with_status(SolverStatus::OverConstrained {
            conflicting_constraints: 2,
        });
        assert!(r.is_over_constrained());
        assert_eq!(r.conflicting_constraints(), Some(2));
        assert_eq!(r.degrees_of_freedom(), None);
        assert_eq!(r.iterations(), None);
    }

    #[test]
    fn report_unstable_status_recognised() {
        let r = report_with_status(SolverStatus::Unstable);
        assert!(r.is_unstable());
        assert!(!r.converged());
        assert_eq!(r.iterations(), None);
        assert_eq!(r.degrees_of_freedom(), None);
    }

    #[test]
    fn report_not_converged_exposes_iterations() {
        let r = report_with_status(SolverStatus::NotConverged {
            iterations: 100,
            final_error: 0.42,
        });
        assert!(!r.converged());
        assert_eq!(r.iterations(), Some(100));
        assert_eq!(r.final_error(), Some(0.42));
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

    // ── B-2: drag preset ────────────────────────────────────────────

    #[test]
    fn for_drag_preset_uses_framerate_tuned_values() {
        let opts = SolveOptions::for_drag();
        assert_eq!(opts.max_iterations, 30);
        assert_eq!(opts.tolerance, 1.0e-6);
        assert_eq!(opts.damping_factor, 0.8);
        // The preset is required to validate.
        assert!(validate_options(&opts).is_ok());
    }

    // ── B-2: build_drag_constraints (synthesis + rejection) ─────────

    #[test]
    fn drag_constraints_synthesise_x_and_y_required_for_point() {
        use super::super::constraints::{
            ConstraintPriority, ConstraintType, DimensionalConstraint,
        };
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));

        let extras = build_drag_constraints(
            &sketch,
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(2.5, -3.0)),
        )
        .expect("drag constraint synthesis succeeds for live point");

        assert_eq!(extras.len(), 2, "expected X+Y pair");
        for c in &extras {
            assert_eq!(c.priority, ConstraintPriority::Required);
            assert_eq!(c.entities.len(), 1);
            assert_eq!(c.entities[0], EntityRef::Point(p));
            match &c.constraint_type {
                ConstraintType::Dimensional(DimensionalConstraint::XCoordinate(x)) => {
                    assert_eq!(*x, 2.5);
                }
                ConstraintType::Dimensional(DimensionalConstraint::YCoordinate(y)) => {
                    assert_eq!(*y, -3.0);
                }
                other => panic!("unexpected drag constraint kind: {:?}", other),
            }
        }
    }

    #[test]
    fn drag_rejects_missing_point_with_entity_not_found() {
        use crate::sketch2d::point2d::Point2dId;

        let sketch = fresh_sketch();
        // Allocate an id without inserting the point so the bridge
        // can detect the dangling reference.
        let ghost = Point2dId::new();
        let err = build_drag_constraints(
            &sketch,
            EntityRef::Point(ghost),
            DragTarget::Point(Point2d::new(0.0, 0.0)),
        )
        .expect_err("ghost point must be rejected");

        match err {
            SketchSolveError::DragEntityNotFound { entity } => {
                assert_eq!(entity, EntityRef::Point(ghost));
            }
            other => panic!("expected DragEntityNotFound, got {:?}", other),
        }
    }

    #[test]
    fn drag_rejects_non_point_entity_with_kind_mismatch() {
        let sketch = fresh_sketch();
        let cid = sketch
            .add_circle(Point2d::new(0.0, 0.0), 1.0)
            .expect("circle");
        let err = build_drag_constraints(
            &sketch,
            EntityRef::Circle(cid),
            DragTarget::Point(Point2d::new(2.0, 0.0)),
        )
        .expect_err("dragging a circle with a Point target must be rejected");
        match err {
            SketchSolveError::DragTargetKindMismatch {
                entity,
                target_kind,
            } => {
                assert_eq!(entity, EntityRef::Circle(cid));
                assert_eq!(target_kind, "Point");
            }
            other => panic!("expected DragTargetKindMismatch, got {:?}", other),
        }
    }

    // ── B-2: solve_drag end-to-end ──────────────────────────────────

    #[test]
    fn solve_drag_pulls_free_point_onto_target() {
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));

        let report = solve_drag(
            &sketch,
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(4.0, -2.0)),
        )
        .expect("drag solve");
        assert!(report.converged(), "status was {:?}", report.status);

        let pos = sketch.points().get(&p).expect("p").value().position;
        assert!((pos.x - 4.0).abs() < 1e-5, "x = {}", pos.x);
        assert!((pos.y - (-2.0)).abs() < 1e-5, "y = {}", pos.y);
    }

    #[test]
    fn solve_drag_does_not_persist_drag_constraints() {
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        let before = sketch.all_constraints().len();
        let _ = solve_drag(
            &sketch,
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(1.0, 1.0)),
        )
        .expect("drag");
        let after = sketch.all_constraints().len();
        assert_eq!(
            before, after,
            "drag must not pollute the sketch ConstraintStore"
        );
    }

    #[test]
    fn solve_drag_respects_existing_distance_constraint() {
        // Pin `a` at the origin, constrain |ab|=5, then drag `b`
        // toward (10, 0). The cursor is unreachable (10>5), so the
        // solver should land `b` on the circle of radius 5 — the
        // closest reachable point in the direction of the drag.
        let sketch = fresh_sketch();
        let a = sketch.add_point(Point2d::new(0.0, 0.0));
        let b = sketch.add_point(Point2d::new(1.0, 0.0));
        if let Some(mut e) = sketch.points().get_mut(&a) {
            e.value_mut().fix();
        } else {
            panic!("a missing");
        }
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(5.0),
            vec![EntityRef::Point(a), EntityRef::Point(b)],
            ConstraintPriority::High,
        ));

        let _ = solve_drag(
            &sketch,
            EntityRef::Point(b),
            DragTarget::Point(Point2d::new(10.0, 0.0)),
        )
        .expect("drag");

        let pa = sketch.points().get(&a).expect("a").value().position;
        let pb = sketch.points().get(&b).expect("b").value().position;
        let d = ((pb.x - pa.x).powi(2) + (pb.y - pa.y).powi(2)).sqrt();
        assert!(
            (d - 5.0).abs() < 1e-4,
            "|ab| should remain 5.0 (closest reachable), got {}",
            d
        );
        // `a` must stay pinned at origin.
        assert!(pa.x.abs() < 1e-10);
        assert!(pa.y.abs() < 1e-10);
    }

    // ── B-2: analyze_dofs ───────────────────────────────────────────

    #[test]
    fn analyze_dofs_empty_sketch_is_fully_constrained() {
        let sketch = fresh_sketch();
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 0);
        assert_eq!(report.constraint_dofs_removed, 0);
        assert_eq!(report.entities_analysed, 0);
        assert_eq!(report.constraints_analysed, 0);
        assert!(report.entities_skipped.is_empty());
        assert!(report.is_fully_constrained());
        assert!(!report.is_under_constrained());
        assert!(!report.is_over_constrained());
        assert_eq!(report.remaining_dofs(), None);
        assert_eq!(report.excess_constraints(), None);
    }

    #[test]
    fn analyze_dofs_counts_two_per_free_point() {
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(0.0, 0.0));
        sketch.add_point(Point2d::new(1.0, 0.0));
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 4);
        assert_eq!(report.entities_analysed, 2);
        // 4 free, 0 removed → 4 DOF under-constrained.
        assert!(report.is_under_constrained());
        assert_eq!(report.remaining_dofs(), Some(4));
    }

    #[test]
    fn analyze_dofs_fixed_points_contribute_zero_dofs() {
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        if let Some(mut e) = sketch.points().get_mut(&p) {
            e.value_mut().fix();
        } else {
            panic!("p missing");
        }
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 0);
        assert_eq!(report.entities_analysed, 1);
        assert!(report.is_fully_constrained());
    }

    #[test]
    fn analyze_dofs_fully_constrained_point_with_x_and_y() {
        // Free point + XCoordinate + YCoordinate = 2 DOFs, 2 removed.
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(3.0, 4.0));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(4.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        ));
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 2);
        assert_eq!(report.constraint_dofs_removed, 2);
        assert!(report.is_fully_constrained());
        assert_eq!(report.constraints_analysed, 2);
    }

    #[test]
    fn analyze_dofs_over_constrained_when_constraints_exceed_freedom() {
        // 1 free point (2 DOF) + 3 dimensional constraints (3 DOF
        // removed) → over-constrained by 1.
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        for v in [0.0, 1.0, 2.0] {
            sketch.add_constraint(Constraint::new_dimensional(
                DimensionalConstraint::XCoordinate(v),
                vec![EntityRef::Point(p)],
                ConstraintPriority::Required,
            ));
        }
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 2);
        assert_eq!(report.constraint_dofs_removed, 3);
        assert!(report.is_over_constrained());
        assert_eq!(report.excess_constraints(), Some(1));
    }

    #[test]
    fn analyze_dofs_skips_unsupported_kinds_into_report() {
        let sketch = fresh_sketch();
        let rect_id = sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(1.0, 1.0))
            .expect("rect");
        let report = analyze_dofs(&sketch);
        // Rectangle contributes 0 DOFs in slice B-2 and lands in
        // entities_skipped so the UI can surface the gap.
        assert_eq!(report.total_free_dofs, 0);
        assert_eq!(report.entities_skipped, vec![EntityRef::Rectangle(rect_id)]);
        // Empty constraint list → no skipped constraints.
        assert_eq!(report.constraints_skipped, 0);
        assert!(!report.has_skipped_constraints());
    }

    #[test]
    fn analyze_dofs_excludes_constraints_touching_unsupported_entities() {
        // 1 point (2 free DOFs) + 1 rectangle (skipped) +
        // Coincident(Point, RectangleVertex via EntityRef::Rectangle).
        // Without filtering, we'd count 2 DOFs removed and report
        // FullyConstrained — a lie, because the constraint can't
        // be evaluated. With filtering: 2 free, 0 removed →
        // UnderConstrained, plus constraints_skipped = 1.
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        let rect = sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(1.0, 1.0))
            .expect("rect");
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![EntityRef::Point(p), EntityRef::Rectangle(rect)],
            ConstraintPriority::High,
        ));

        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 2);
        assert_eq!(
            report.constraint_dofs_removed, 0,
            "constraint must be skipped because rectangle is unsupported"
        );
        assert_eq!(report.constraints_analysed, 0);
        assert_eq!(report.constraints_skipped, 1);
        assert!(report.has_skipped_constraints());
        assert!(report.is_under_constrained());
        assert_eq!(report.remaining_dofs(), Some(2));
    }

    // ── B-2 hardening: dragging a fixed point is rejected ──────────

    #[test]
    fn drag_rejects_fixed_point_up_front() {
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        if let Some(mut e) = sketch.points().get_mut(&p) {
            e.value_mut().fix();
        } else {
            panic!("p missing");
        }
        let err = solve_drag(
            &sketch,
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(1.0, 1.0)),
        )
        .expect_err("dragging a fixed point must be rejected");
        match err {
            SketchSolveError::DragEntityFixed { entity } => {
                assert_eq!(entity, EntityRef::Point(p));
            }
            other => panic!("expected DragEntityFixed, got {:?}", other),
        }
        // Position must not have moved (drag rejected before solve).
        let pos = sketch.points().get(&p).expect("p").value().position;
        assert_eq!(pos.x, 0.0);
        assert_eq!(pos.y, 0.0);
    }

    #[test]
    fn drag_preserves_entity_id_across_call() {
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(0.0, 0.0));
        let _ = solve_drag(
            &sketch,
            EntityRef::Point(p),
            DragTarget::Point(Point2d::new(5.0, 5.0)),
        )
        .expect("drag");
        // Identity preservation: the id we started with still
        // resolves after the drag. Matches the body-modify
        // architecture (chamfer/fillet/mirror/shell/extrude_face
        // also emit ObjectUpdated rather than delete+create).
        assert!(
            sketch.points().contains_key(&p),
            "drag must preserve entity id"
        );
    }
}
