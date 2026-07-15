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
//! [`solve`] translates the sketch's points, lines, circles, arcs,
//! rectangles, ellipses, and splines (both non-rational B-Spline
//! and rational NURBS) into solver `EntityState`s, runs the
//! existing solver, and writes the result back onto the parametric
//! entities. Polylines pass through unsolved (slice C-5 lifts the
//! remaining kind).
//!
//! For LEGACY arcs (no shared refs) the bridge solves the 5-parameter
//! state `[center.x, center.y, radius, start_angle, end_angle]`.
//! Shared-variable arcs and circles (SKETCH-DCM #45 Slice 1) register
//! as derived entities instead: an endpoint-derived arc owns one
//! chord-offset parameter (see `EntityState::arc_between`), a
//! shared-center arc owns `[r, a0, a1]`, and a shared-center circle
//! owns `[r]` — the referenced points carry the positional DOFs. In
//! every case the `ccw` orientation bit is preserved across solve
//! cycles in [`apply_solver_result`] — it is not a
//! continuously-varying parameter the solver could differentiate
//! over.
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
use super::ellipse2d::{Ellipse2d, ParametricEllipse2d};
use super::line2d::{LineGeometry, ParametricLine2d};
use super::point2d::ParametricPoint2d;
use super::polyline2d::Polyline2d;
use super::rectangle2d::{ParametricRectangle2d, Rectangle2d};
use super::sketch::Sketch;
use super::spline2d::{BSpline2d, NurbsCurve2d, ParametricSpline2d, Spline2d};
use super::{Circle2d, Line2d, LineSegment2d, Point2d, Ray2d, Vector2d};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Tag for an entity the bridge skipped because the solver does not yet
/// have a public `EntityState` constructor for its kind.
///
/// Surfaced in [`SketchSolveReport::entities_skipped`] so the UI can
/// highlight specifically which polylines will remain unsolved until
/// slice C-5 lands.
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
    /// before invoking it (= points + lines + circles + arcs +
    /// rectangles).
    pub entities_solved: usize,
    /// Number of constraints registered with the solver. Includes
    /// constraints whose entity kind is unsupported in v1 (they are
    /// no-ops inside the solver but still counted).
    pub constraints_solved: usize,
    /// Entity references the bridge could not address through the
    /// public `EntityState` constructors and therefore excluded from
    /// the solve. As of slice C-4 the remaining unsupported kind is
    /// polylines (slice C-5).
    /// Reporting the [`EntityRef`] (not just a count) lets the UI
    /// highlight exactly which entities went unsolved.
    pub entities_skipped: Vec<SkippedEntity>,
}

impl SketchSolveReport {
    /// Convenience: did the solver find a configuration that
    /// satisfies every constraint to within tolerance?
    ///
    /// True for `SolverStatus::Converged`. Also true for
    /// `SolverStatus::UnderConstrained` when `violations.is_empty()`:
    /// the constraint solver runs Tikhonov-regularised Newton even
    /// on under-constrained systems (see
    /// [`ConstraintSolver::solve`] doc), so a degenerate Jacobian
    /// still yields a minimum-norm step that can drive every
    /// residual under `self.tolerance` — at which point
    /// `get_violations` returns the empty list. This is the
    /// "Solved" state in Onshape / SolidWorks / Fusion parlance
    /// (distinct from "Fully Defined", which additionally requires
    /// zero remaining DOFs — see [`Self::is_fully_constrained`]).
    pub fn converged(&self) -> bool {
        match self.status {
            SolverStatus::Converged { .. } => true,
            SolverStatus::UnderConstrained { .. } => self.violations.is_empty(),
            SolverStatus::OverConstrained { .. }
            | SolverStatus::NotConverged { .. }
            | SolverStatus::Unstable => false,
        }
    }

    /// Convenience: is the sketch fully constrained (converged with
    /// no remaining degrees of freedom)?
    ///
    /// In Onshape / SolidWorks / Fusion parlance this is the "fully
    /// defined" state that turns the sketch border green. Strictly
    /// stronger than [`Self::converged`]: an under-constrained
    /// sketch can be "Solved" (all residuals satisfied) without
    /// being "Fully Defined" (free DOFs remain).
    pub fn is_fully_constrained(&self) -> bool {
        matches!(self.status, SolverStatus::Converged { .. })
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
    /// constructor yet (polyline).
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
    /// Lines contribute 4 (px, py, dx, dy); endpoint-derived
    /// segments contribute 0 (their geometry IS their points).
    /// Circles contribute 3 (cx, cy, r); shared-center circles
    /// contribute 1 (radius only — the center is the point's).
    /// Arcs contribute 5 (cx, cy, r, start_angle, end_angle);
    /// endpoint-derived arcs contribute 1 (chord offset) and
    /// shared-center arcs 3 (r, a0, a1) — SKETCH-DCM #45 Slice 1.
    /// Rectangles contribute 5 (cx, cy, width, height, rotation).
    /// Ellipses contribute 5 (cx, cy, semi_major, semi_minor, rotation).
    /// Splines (B-Spline and NURBS) contribute 2n where n is the
    /// control-point count — knots and weights are pinned as
    /// structural metadata, so only CP coordinates are free DOFs.
    /// Entities listed in `entities_skipped` contribute 0 — slice
    /// C-5 lifts this for polylines.
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
    /// view of the sketch — slice C-5 lifts the remaining gap for
    /// polylines.
    pub constraints_skipped: usize,
    /// Entities that cannot contribute DOFs in slice B-2 — same
    /// list semantics as [`SketchSolveReport::entities_skipped`].
    pub entities_skipped: Vec<SkippedEntity>,
    /// Constraints whose Jacobian rows are linearly dependent on
    /// earlier rows **and** whose post-solve residual is within
    /// tolerance. Safe-to-remove duplicates: the system still has a
    /// solution, but this constraint contributes no new information.
    ///
    /// Populated when the structural verdict is not
    /// `FullyConstrained` (i.e. one of `OverConstrained` /
    /// `UnderConstrained`). For `FullyConstrained` sketches the
    /// numerical analysis is skipped because the structural count
    /// already proves there is no redundancy.
    ///
    /// Order is deterministic for any given constraint ordering: the
    /// list reflects the first time each redundant constraint was
    /// encountered while scanning rows of the Jacobian.
    #[serde(default)]
    pub redundant: Vec<ConstraintId>,
    /// Constraints whose Jacobian rows are linearly dependent on
    /// earlier rows **and** whose post-solve residual exceeds
    /// tolerance. These are part of an inconsistent subset — the
    /// sketch cannot satisfy them simultaneously with the
    /// independent constraints that came before.
    ///
    /// Same population rules as `redundant`. The classification is
    /// order-dependent: a true global minimum-cardinality unsat
    /// subset (MUS) requires QuickXplain-style search, deferred to a
    /// follow-up slice.
    #[serde(default)]
    pub conflicts: Vec<ConstraintId>,
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
/// The drag is implemented as weighted least-squares: a temporary
/// `Low`-priority `XCoordinate(target.x)` and `YCoordinate(target.y)`
/// pair is added to the constraint set for this solve invocation
/// only — they are not persisted into the sketch's
/// `ConstraintStore`. Because they are `Low` priority, the solver's
/// priority weighting subordinates them to any pre-existing
/// `Required`/`High` constraint: if the cursor target is reachable
/// the dragged point lands on it, but if not (e.g. the point is
/// constrained to a fixed circle and the cursor is off the circle),
/// Newton-Raphson honours the rigid constraint exactly and lands on
/// the closest reachable point in the direction of the drag. This
/// matches the drag behaviour every modern parametric sketcher
/// implements.
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
    // Lines: 4 DOFs (px, py, dx, dy) for an INDEPENDENT line. A segment
    // derived from two endpoint points (the shared-variable model,
    // `endpoints = Some(..)`) owns ZERO independent DOF — its geometry IS its
    // endpoints, already counted above. Counting a derived segment as 4 phantom
    // DOF left every sketch that contains a segment perpetually 4-DOF
    // under-constrained, so `FullyConstrained` could never be reported. The
    // solver's `populate_solver` already treats derived segments as parameter-
    // free, so this aligns the DOF count with what Newton-Raphson iterates.
    for entry in sketch.lines().iter() {
        entities_analysed += 1;
        if entry.value().endpoints.is_none() {
            total_free_dofs += 4;
        }
    }
    // Circles: 3 DOFs (cx, cy, r) for an INDEPENDENT circle. A circle
    // whose center is a shared point (SKETCH-DCM #45 Slice 1,
    // `center_point = Some(..)` and the point still exists) owns ONE
    // private DOF — the radius; its center coordinates are counted
    // once, in the point, above. The classification is shared with
    // `populate_solver` (`circle_shared_center`) so the structural
    // count matches the solver's parameter vector exactly.
    for entry in sketch.circles().iter() {
        entities_analysed += 1;
        total_free_dofs += if circle_shared_center(sketch, entry.value()).is_some() {
            1
        } else {
            3
        };
    }
    // Arcs: 5 DOFs (cx, cy, r, start_angle, end_angle) for an
    // INDEPENDENT arc. Shared-variable arcs (SKETCH-DCM #45 Slice 1)
    // contribute only their PRIVATE parameters — the shared points
    // are counted above:
    //   - endpoint-derived arc: 1 DOF (center offset along the
    //     chord's perpendicular bisector; radius/center/angles all
    //     derive from the two shared points + that offset);
    //   - shared-center arc: 3 DOF (r, a0, a1).
    // Counting a shared arc as 5 would double-book its endpoint /
    // center coordinates and leave every slot profile permanently
    // "under-constrained" — the same phantom-DOF bug the derived
    // segment fix above killed for lines. The classification is
    // `arc_solver_mode`, shared with `populate_solver`, so the count
    // matches what Newton-Raphson actually iterates. `ccw` is not a
    // solver DOF — orientation is a discrete bit.
    for entry in sketch.arcs().iter() {
        entities_analysed += 1;
        total_free_dofs += match arc_solver_mode(sketch, entry.value()) {
            ArcSolverMode::SharedEndpoints { .. } => 1,
            ArcSolverMode::SharedCenter { .. } => 3,
            ArcSolverMode::Legacy => 5,
        };
    }
    // Rectangles: 5 DOFs (cx, cy, width, height, rotation). Matches
    // the solver-side `EntityState::rectangle` parameter layout
    // introduced in slice C-2. `ParametricRectangle2d` carries no
    // per-DOF fix flag today; the four corners are derived from
    // (center, width, height, rotation) on every read.
    for _entry in sketch.rectangles().iter() {
        entities_analysed += 1;
        total_free_dofs += 5;
    }
    // Ellipses: 5 DOFs (cx, cy, semi_major, semi_minor, rotation).
    // Matches the solver-side `EntityState::ellipse` parameter
    // layout introduced in slice C-3. Counted as 5 regardless of
    // orientation — `ParametricEllipse2d::degrees_of_freedom`
    // returns 4 for axis-aligned ellipses, but the solver always
    // carries 5 free parameters and a user who wants axis-alignment
    // adds an explicit Horizontal constraint on the major axis. The
    // alternative — branching on a near-zero rotation — would
    // produce a flicker in the DOF badge as the rotation drifts
    // across the tolerance boundary during solving.
    for _entry in sketch.ellipses().iter() {
        entities_analysed += 1;
        total_free_dofs += 5;
    }
    // Splines (B-Spline and NURBS): 2 DOFs per control point.
    // Mirrors the solver-side `EntityState::spline_{bspline,nurbs}`
    // parameter pack registered by `populate_solver` — knots and
    // (for NURBS) weights are pinned in `SplineMetadata` and never
    // become free DOFs of Newton-Raphson. The 2n count therefore
    // exactly matches what the solver will iterate.
    for entry in sketch.splines().iter() {
        entities_analysed += 1;
        let cp_count = match &entry.value().spline {
            Spline2d::BSpline(bs) => bs.control_points.len(),
            Spline2d::Nurbs(nurbs) => nurbs.control_points.len(),
        };
        total_free_dofs += 2 * cp_count;
    }
    // Polylines: 2 DOFs per vertex.
    // Mirrors the solver-side `EntityState::polyline` parameter pack
    // registered by `populate_solver` — `is_closed` is pinned in
    // `PolylineMetadata` and never becomes a free DOF of
    // Newton-Raphson. The 2n count exactly matches
    // `ParametricPolyline2d::degrees_of_freedom`.
    for entry in sketch.polylines().iter() {
        entities_analysed += 1;
        total_free_dofs += 2 * entry.value().polyline.vertices.len();
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

    // Numerical diagnostic pass: when the structural count says the
    // sketch is anything other than `FullyConstrained`, classify each
    // constraint as essential / redundant / conflicting using the
    // Jacobian's rank profile. The pass is skipped for fully
    // constrained sketches because by construction `constraint_dofs
    // _removed == total_free_dofs` precludes both redundancy and
    // conflict.
    //
    // The diagnostic builds an isolated `ConstraintSolver` populated
    // from the sketch's *current* entity state (which may or may not
    // be a converged solution). The solver's own DashMap is the only
    // mutable surface; the sketch is never written back. This keeps
    // `analyze_dofs` side-effect-free, matching its documented
    // contract.
    // A constraint the solver recognises but cannot enforce (e.g.
    // MinDistance, MomentOfInertia) removes zero DOF, so on its own it
    // can leave a sketch looking structurally `FullyConstrained` while a
    // constraint is being silently ignored. Detect any such constraint
    // on supported entities and force the numerical diagnosis to run —
    // the solver emits an irreducible residual for it, which surfaces as
    // a conflict so the certificate refuses to call the sketch
    // consistent rather than falsely certifying a solve.
    let has_unenforced = sketch.all_constraints().iter().any(|c| {
        !c.constraint_type.is_numerically_enforced()
            && !c.entities.iter().any(|e| skipped_set.contains(e))
    });
    let (redundant, conflicts) = match status {
        DofStatus::FullyConstrained if !has_unenforced => (Vec::new(), Vec::new()),
        _ => diagnose_constraints(sketch, &skipped_set),
    };

    DofReport {
        total_free_dofs,
        constraint_dofs_removed,
        status,
        entities_analysed,
        constraints_analysed,
        constraints_skipped,
        entities_skipped,
        redundant,
        conflicts,
    }
}

/// Build a non-mutating `ConstraintSolver` snapshot of the sketch
/// and run its rank-revealing diagnosis. Only constraints whose
/// entire entity set is supported by the bridge are fed to the
/// solver — the rest contribute neither rows nor verdict so the
/// classification stays consistent with the DOF accounting above.
///
/// Returns `(redundant, conflicts)` constraint id lists per
/// `ConstraintDiagnosis::redundant` / `::conflicts` semantics.
fn diagnose_constraints(
    sketch: &Sketch,
    skipped_set: &std::collections::HashSet<EntityRef>,
) -> (Vec<ConstraintId>, Vec<ConstraintId>) {
    let mut solver = ConstraintSolver::new();
    populate_solver(sketch, &solver);

    let mut diagnosable: Vec<super::constraints::Constraint> = Vec::new();
    for c in sketch.all_constraints().iter() {
        if c.entities.iter().any(|e| skipped_set.contains(e)) {
            continue;
        }
        diagnosable.push(c.clone());
    }
    if diagnosable.is_empty() {
        return (Vec::new(), Vec::new());
    }
    // `sketch.all_constraints()` iterates a `DashMap` and is therefore
    // unordered. The diagnosis is order-dependent (the first row in
    // a linearly-dependent set is the "essential" one; the rest are
    // flagged). Sort by id so the verdict is deterministic across
    // calls — the UI can otherwise see the same sketch produce
    // different redundancy lists on consecutive `/dof` requests.
    diagnosable.sort_by_key(|c| c.id.0);
    solver.set_constraints(diagnosable);

    // Run Newton-Raphson before `diagnose()` so the residual readings
    // reflect the post-solve state, not the entities' freshly-loaded
    // initial positions. Without this, a constraint that happens to
    // be satisfied by the initial guess (residual = 0) is classified
    // as *redundant* even when it is part of an inconsistent set —
    // e.g. three `XCoordinate` constraints with values {3, 7, 9} on
    // a point initially at x=3. The order of constraint processing
    // (deterministic, by `id` sort above) then picks which row is
    // the "essential" representative; running `solve()` first pushes
    // the point to the regularised least-squares minimum so every
    // dependent row carries a non-zero residual and the conflict
    // classifier produces the same count regardless of which uuid
    // sorts first. Matches the `diagnose()` contract:
    // "callers that need a residual-accurate solution should run
    // solve() before diagnose()".
    //
    // The solver mutates only its internal `entity_state`; the
    // sketch is never written back, so `analyze_dofs` stays
    // side-effect-free.
    let _ = solver.solve();
    let diagnosis = solver.diagnose();
    (diagnosis.redundant, diagnosis.conflicts)
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
            // Synthesised drag constraints are intentionally `Low`
            // priority — soft pulls, not rigid pins. The constraint
            // solver's priority-weighted least-squares treats them
            // as dominated by any `Required`/`High` constraint the
            // sketch already carries, so a drag toward an
            // unreachable cursor lands on the closest reachable
            // point (e.g. the intersection of the drag direction
            // with a distance circle). This matches Fusion /
            // SolveSpace drag semantics.
            let x_constraint = Constraint::new_dimensional(
                DimensionalConstraint::XCoordinate(target_pos.x),
                vec![EntityRef::Point(point_id)],
                ConstraintPriority::Low,
            );
            let y_constraint = Constraint::new_dimensional(
                DimensionalConstraint::YCoordinate(target_pos.y),
                vec![EntityRef::Point(point_id)],
                ConstraintPriority::Low,
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

/// How the solver bridge registers an arc (SKETCH-DCM #45 Slice 1).
///
/// The classifier is THE single source of truth shared by
/// [`populate_solver`] (which parameters the solver iterates) and
/// [`analyze_dofs`] (how many DOFs the arc contributes) — computing
/// the mode in one place is what keeps the structural DOF count and
/// the Jacobian's column count in lock-step.
enum ArcSolverMode {
    /// Endpoints are shared points: 1 private DOF (the center's
    /// signed offset along the chord's perpendicular bisector —
    /// see `EntityState::arc_between`).
    SharedEndpoints {
        start: super::Point2dId,
        end: super::Point2dId,
        center_offset: f64,
    },
    /// Center is a shared point: 3 private DOF (r, a0, a1).
    SharedCenter { center: super::Point2dId },
    /// No shared refs (or refs unusable): private 5-parameter arc,
    /// byte-identical to pre-Slice-1 behaviour.
    Legacy,
}

/// Classify an arc's solver registration mode.
///
/// Shared endpoints take PRECEDENCE over a shared center (both set is
/// unreachable through the creation API; honouring both would need an
/// implicit equidistance residual — refused, see
/// `ParametricArc2d::center_point`). A shared ref degrades to the
/// next mode when its point has been deleted, or — for endpoints —
/// when the chord is degenerate (points coincident within
/// `STRICT_TOLERANCE`), because the chord-offset parameterization has
/// no frame there.
fn arc_solver_mode(sketch: &Sketch, arc: &ParametricArc2d) -> ArcSolverMode {
    use crate::math::tolerance::STRICT_TOLERANCE;

    if let Some((start_id, end_id)) = arc.endpoints {
        if let (Some(s), Some(e)) = (sketch.get_point(&start_id), sketch.get_point(&end_id)) {
            let chord = Vector2d::new(e.x - s.x, e.y - s.y);
            let chord_len = chord.magnitude();
            if chord_len >= STRICT_TOLERANCE.distance() {
                // Project the CURRENT center onto the chord frame to
                // seed the offset parameter: t = (C − M) · p̂ with
                // p̂ the chord's left-hand unit perpendicular. The
                // round-trip (write-back computes C = M + t·p̂) is the
                // identity for a center already on the bisector, and
                // snaps a stale center onto it otherwise — the arc
                // cannot disagree with its shared points.
                let u = Vector2d::new(chord.x / chord_len, chord.y / chord_len);
                let perp = Vector2d::new(-u.y, u.x);
                let mid_x = (s.x + e.x) / 2.0;
                let mid_y = (s.y + e.y) / 2.0;
                let center_offset =
                    (arc.arc.center.x - mid_x) * perp.x + (arc.arc.center.y - mid_y) * perp.y;
                return ArcSolverMode::SharedEndpoints {
                    start: start_id,
                    end: end_id,
                    center_offset,
                };
            }
        }
    }
    if let Some(center_id) = arc.center_point {
        if sketch.points().contains_key(&center_id) {
            return ArcSolverMode::SharedCenter { center: center_id };
        }
    }
    ArcSolverMode::Legacy
}

/// Shared-center ref of a circle, iff the referenced point still
/// exists. Shared by [`populate_solver`] and [`analyze_dofs`] — same
/// lock-step rationale as [`arc_solver_mode`].
fn circle_shared_center(sketch: &Sketch, circle: &ParametricCircle2d) -> Option<super::Point2dId> {
    circle
        .center_point
        .filter(|id| sketch.points().contains_key(id))
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
        solver.add_entity(
            EntityRef::Point(id),
            EntityState::point(p.position, p.is_fixed),
        );
        registered += 1;
    }

    for entry in sketch.lines().iter() {
        let id = *entry.key();
        let line: &ParametricLine2d = entry.value();
        // SHARED-VARIABLE MODEL (SKETCH-DCM A.2): a segment that knows
        // its endpoint points registers as a DERIVED entity — zero own
        // DOF, geometry computed from the points' live solver state.
        // This is what couples Horizontal(line) to Distance(point,
        // point): both now differentiate against the same variables.
        // Guard on both points still existing; a segment whose points
        // were deleted degrades to the legacy private-geometry path.
        if let (LineGeometry::Segment(_), Some((start_id, end_id))) =
            (&line.geometry, line.endpoints)
        {
            if sketch.points().contains_key(&start_id) && sketch.points().contains_key(&end_id) {
                solver.add_entity(
                    EntityRef::Line(id),
                    EntityState::segment_between(
                        EntityRef::Point(start_id),
                        EntityRef::Point(end_id),
                    ),
                );
                registered += 1;
                continue;
            }
        }
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
        // SHARED-VARIABLE MODEL (SKETCH-DCM #45 Slice 1): a circle
        // that knows its center point registers as a DERIVED entity —
        // one private DOF (the radius), center read from the point's
        // live solver state. This is what makes two circles on the
        // same point concentric BY CONSTRUCTION, and a drag of the
        // point move every circle centered on it. A circle whose
        // center point was deleted degrades to the legacy 3-parameter
        // path.
        if let Some(center_id) = circle_shared_center(sketch, circle) {
            solver.add_entity(
                EntityRef::Circle(id),
                EntityState::circle_centered(EntityRef::Point(center_id), circle.circle.radius),
            );
            registered += 1;
            continue;
        }
        solver.add_entity(
            EntityRef::Circle(id),
            EntityState::circle(circle.circle.center, circle.circle.radius, false, false),
        );
        registered += 1;
    }

    for entry in sketch.arcs().iter() {
        let id = *entry.key();
        let arc: &ParametricArc2d = entry.value();
        // SHARED-VARIABLE MODEL (SKETCH-DCM #45 Slice 1): arcs with
        // shared refs register as DERIVED entities — see
        // [`arc_solver_mode`] for the classification and
        // `EntityState::arc_between` for the chord-offset
        // parameterization. `ccw` is never a solver parameter
        // (orientation is a discrete bit, not a continuously-varying
        // scalar); the bridge preserves it across solve cycles in
        // [`apply_solver_result`].
        match arc_solver_mode(sketch, arc) {
            ArcSolverMode::SharedEndpoints {
                start,
                end,
                center_offset,
            } => {
                solver.add_entity(
                    EntityRef::Arc(id),
                    EntityState::arc_between(
                        EntityRef::Point(start),
                        EntityRef::Point(end),
                        center_offset,
                    ),
                );
            }
            ArcSolverMode::SharedCenter { center } => {
                solver.add_entity(
                    EntityRef::Arc(id),
                    EntityState::arc_centered(
                        EntityRef::Point(center),
                        arc.arc.radius,
                        arc.arc.start_angle,
                        arc.arc.end_angle,
                    ),
                );
            }
            ArcSolverMode::Legacy => {
                // Arcs have no per-DOF fix flags today — pass `false`
                // for every group so the solver treats
                // center/radius/angles as free unless a dimensional
                // or positional constraint pins them.
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
            }
        }
        registered += 1;
    }

    for entry in sketch.rectangles().iter() {
        let id = *entry.key();
        let rect: &ParametricRectangle2d = entry.value();
        // Rectangles have no per-DOF fix flags on the parametric
        // wrapper (only `is_construction`), matching the line / circle
        // / arc convention. All five DOFs (center.x, center.y, width,
        // height, rotation) start free; explicit dimensional or
        // positional constraints can pin them downstream.
        solver.add_entity(
            EntityRef::Rectangle(id),
            EntityState::rectangle(
                rect.rectangle.center,
                rect.rectangle.width,
                rect.rectangle.height,
                rect.rectangle.rotation,
                false,
                false,
                false,
                false,
            ),
        );
        registered += 1;
    }

    for entry in sketch.ellipses().iter() {
        let id = *entry.key();
        let ellipse: &ParametricEllipse2d = entry.value();
        // Ellipses have no per-DOF fix flags on the parametric
        // wrapper (only `is_construction`), matching every other
        // supported kind. All five DOFs (center.x, center.y,
        // semi_major, semi_minor, rotation) start free; explicit
        // dimensional or positional constraints can pin them
        // downstream. The `semi_major >= semi_minor` convention is
        // enforced only on write-back through `Ellipse2d::new`, not
        // inside the solver — keeping the two axes independent
        // floats during iteration keeps the Jacobian well-conditioned.
        solver.add_entity(
            EntityRef::Ellipse(id),
            EntityState::ellipse(
                ellipse.ellipse.center,
                ellipse.ellipse.semi_major,
                ellipse.ellipse.semi_minor,
                ellipse.ellipse.rotation,
                false,
                false,
                false,
                false,
            ),
        );
        registered += 1;
    }

    for entry in sketch.splines().iter() {
        let id = *entry.key();
        let spline: &ParametricSpline2d = entry.value();
        // Spline registration (slice C-4-b). The solver's parameter
        // pack is 2n entries regardless of rationality — weights and
        // knots are pinned in `SplineMetadata`, so the solver only
        // moves control-point coordinates. NURBS uses the rational
        // dispatch path (`NurbsCurve2d::closest_point` + `tangent`);
        // B-Spline uses the non-rational path. Per-CP fix flags are
        // not yet exposed; every CP starts free and can be pinned by
        // explicit Coincident-to-fixed-point constraints downstream.
        let state = match &spline.spline {
            Spline2d::BSpline(bs) => EntityState::spline_bspline(
                bs.degree,
                bs.control_points.clone(),
                bs.knots.clone(),
                false,
            ),
            Spline2d::Nurbs(nurbs) => EntityState::spline_nurbs(
                nurbs.degree,
                nurbs.control_points.clone(),
                nurbs.weights.clone(),
                nurbs.knots.clone(),
                false,
            ),
        };
        solver.add_entity(EntityRef::Spline(id), state);
        registered += 1;
    }

    for entry in sketch.polylines().iter() {
        let id = *entry.key();
        let polyline = entry.value();
        // Polyline registration (slice C-5). The solver's parameter
        // pack is 2n entries for n vertices; `is_closed` is pinned in
        // `PolylineMetadata` because flipping it is a structural edit
        // (adds or removes the wrap-around segment). Per-vertex fix
        // flags are not yet exposed; every vertex starts free and can
        // be pinned by explicit Coincident-to-fixed-point constraints
        // downstream.
        let state = EntityState::polyline(
            polyline.polyline.vertices.clone(),
            polyline.polyline.is_closed,
            false,
        );
        solver.add_entity(EntityRef::Polyline(id), state);
        registered += 1;
    }

    registered
}

/// Collect `EntityRef`s for entities whose kinds are unsupported by
/// the current bridge.
///
/// As of slice C-5 every sketch-entity kind (Point, Line, Circle, Arc,
/// Rectangle, Ellipse, Spline, Polyline) has an `EntityState`
/// constructor and a registration arm in
/// [`populate_solver`] — this function therefore returns an empty
/// vector. The infrastructure (collection + propagation through
/// `entities_skipped` and `constraints_skipped`) is kept so a future
/// new kind can be added without re-introducing the plumbing: a
/// single `for entry in sketch.<new_kind>s().iter()` push into
/// `skipped` is all that's needed.
///
/// The `_sketch` parameter is intentionally unused right now; renaming
/// the argument (rather than removing it) keeps the signature stable
/// for the future-new-kind path so callers don't need to change.
fn collect_unsupported(_sketch: &Sketch) -> Vec<EntityRef> {
    Vec::new()
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
/// solver does not own it. Shared-variable circles/arcs (SKETCH-DCM
/// #45 Slice 1) arrive through the same `EntityUpdate::Circle` /
/// `EntityUpdate::Arc` arms, but their payloads were COMPUTED from
/// the shared points' solved state inside the solver
/// (`get_entity_updates`), so the written-back geometry agrees with
/// the points by construction — the same single-source-of-truth
/// contract the derived-segment sync pass below enforces for lines.
/// Updates for kinds the bridge did not register (polyline) cannot
/// appear here because we never called `add_entity` for them;
/// defensive matches are still in place to avoid panics if the
/// solver ever starts returning them.
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
                    let new_geometry =
                        apply_line_update(&entry.value().geometry, *point, *direction);
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
            (EntityRef::Arc(id), EntityUpdate::Arc(center, radius, start_angle, end_angle)) => {
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
            (
                EntityRef::Rectangle(id),
                EntityUpdate::Rectangle(center, width, height, rotation),
            ) => {
                if let Some(mut entry) = sketch.rectangles().get_mut(id) {
                    // Use `Rectangle2d::new_rotated` rather than
                    // mutating the struct in place so the
                    // width > tolerance / height > tolerance
                    // invariants live in one place. If the solver
                    // pushes the dimensions through zero (which would
                    // happen on a wildly under-constrained sketch),
                    // the constructor returns Err and we leave the
                    // prior geometry intact — better UX than a
                    // panic and aligns with the "preserve identity,
                    // never poison the store" convention.
                    if let Ok(updated) =
                        Rectangle2d::new_rotated(*center, *width, *height, *rotation)
                    {
                        entry.value_mut().rectangle = updated;
                    }
                }
            }
            (EntityRef::Spline(id), EntityUpdate::Parameters(params)) => {
                if let Some(mut entry) = sketch.splines().get_mut(id) {
                    // Solver returns a flat `[x0, y0, x1, y1, …]`
                    // pack — same layout the registration loop
                    // packed control points into. Degree, knots, and
                    // (for NURBS) weights are pinned in
                    // `SplineMetadata` on the solver side, so they
                    // are preserved here by reading them from the
                    // prior geometry rather than the update payload.
                    //
                    // A parameter-count mismatch (e.g. the solver
                    // returning fewer/more floats than the current
                    // CP count) or a curve-validation failure on
                    // reconstruction leaves the prior geometry
                    // intact — same "preserve identity, never poison
                    // the store" convention used for rectangle /
                    // ellipse write-back.
                    if params.len() % 2 != 0 {
                        continue;
                    }
                    let expected_cps = params.len() / 2;
                    let mut new_ctrl = Vec::with_capacity(expected_cps);
                    for i in 0..expected_cps {
                        let base = i * 2;
                        new_ctrl.push(Point2d::new(params[base], params[base + 1]));
                    }
                    let updated = match &entry.value().spline {
                        Spline2d::BSpline(bs) => {
                            if new_ctrl.len() != bs.control_points.len() {
                                None
                            } else {
                                BSpline2d::new(bs.degree, new_ctrl, bs.knots.clone())
                                    .ok()
                                    .map(Spline2d::BSpline)
                            }
                        }
                        Spline2d::Nurbs(nurbs) => {
                            if new_ctrl.len() != nurbs.control_points.len() {
                                None
                            } else {
                                NurbsCurve2d::new(
                                    nurbs.degree,
                                    new_ctrl,
                                    nurbs.weights.clone(),
                                    nurbs.knots.clone(),
                                )
                                .ok()
                                .map(Spline2d::Nurbs)
                            }
                        }
                    };
                    if let Some(s) = updated {
                        entry.value_mut().spline = s;
                    }
                }
            }
            (EntityRef::Polyline(id), EntityUpdate::Parameters(params)) => {
                if let Some(mut entry) = sketch.polylines().get_mut(id) {
                    // Solver returns a flat `[x0, y0, x1, y1, …]`
                    // pack — same layout the registration loop
                    // packed vertices into. `is_closed` is pinned in
                    // `PolylineMetadata` on the solver side, so it is
                    // preserved here by reading it from the prior
                    // geometry rather than the update payload.
                    //
                    // A parameter-count mismatch (e.g. the solver
                    // returning fewer/more floats than the current
                    // vertex count) or a `Polyline2d::new` validation
                    // failure (coincident consecutive vertices that
                    // the solver drifted into) leaves the prior
                    // geometry intact — same "preserve identity,
                    // never poison the store" convention used for
                    // rectangle / ellipse / spline write-back.
                    if params.len() % 2 != 0 {
                        continue;
                    }
                    let expected = params.len() / 2;
                    let current_len = entry.value().polyline.vertices.len();
                    if expected != current_len {
                        continue;
                    }
                    let mut new_vertices = Vec::with_capacity(expected);
                    for i in 0..expected {
                        let base = i * 2;
                        new_vertices.push(Point2d::new(params[base], params[base + 1]));
                    }
                    let is_closed = entry.value().polyline.is_closed;
                    if let Ok(updated) = Polyline2d::new(new_vertices, is_closed) {
                        entry.value_mut().polyline = updated;
                    }
                }
            }
            (
                EntityRef::Ellipse(id),
                EntityUpdate::Ellipse(center, semi_major, semi_minor, rotation),
            ) => {
                if let Some(mut entry) = sketch.ellipses().get_mut(id) {
                    // Use `Ellipse2d::new` rather than mutating the
                    // struct in place so the
                    // semi_major > tolerance / semi_minor > tolerance
                    // invariants live in one place. If the solver
                    // pushes either axis through zero (which would
                    // happen on a wildly under-constrained sketch),
                    // the constructor returns Err and we leave the
                    // prior geometry intact — same "preserve
                    // identity, never poison the store" convention
                    // used for rectangle write-back. The
                    // `semi_major >= semi_minor` normalisation (and
                    // the 90° rotation adjust when the swap fires)
                    // is also handled by the constructor, so the
                    // store never carries an un-normalised ellipse.
                    if let Ok(updated) =
                        Ellipse2d::new(*center, *semi_major, *semi_minor, *rotation)
                    {
                        entry.value_mut().ellipse = updated;
                    }
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

    // SHARED-VARIABLE MODEL (SKETCH-DCM A.2): after every point update
    // has landed, rebuild each endpoint-linked segment's geometry from
    // its points' SOLVED positions. Derived lines emit no
    // EntityUpdate::Line (they own no parameters), so this pass is the
    // single writer of their geometry — the line cannot disagree with
    // its endpoints, which is the accuracy contract the csketch
    // extrude bridge and every downstream consumer rely on.
    for mut entry in sketch.lines().iter_mut() {
        let Some((start_id, end_id)) = entry.value().endpoints else {
            continue;
        };
        let (Some(a), Some(b)) = (
            sketch.points().get(&start_id).map(|p| p.value().position),
            sketch.points().get(&end_id).map(|p| p.value().position),
        ) else {
            continue;
        };
        if let Ok(segment) = LineSegment2d::new(a, b) {
            entry.value_mut().geometry = LineGeometry::Segment(segment);
        }
    }
}

/// Reconstruct a `LineGeometry` from solved `(point, direction)`.
///
/// For `Infinite` and `Ray` the parameters map straight back. For
/// `Segment` the bridge stored `direction = end - start`, so the
/// reconstructed end is `start + direction`.
fn apply_line_update(prior: &LineGeometry, point: Point2d, direction: Vector2d) -> LineGeometry {
    match prior {
        LineGeometry::Infinite(_) => LineGeometry::Infinite(Line2d { point, direction }),
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
        Constraint, ConstraintId, ConstraintPriority, DimensionalConstraint, GeometricConstraint,
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
        let report =
            solve_with_options(&fresh_sketch(), opts).expect("damping 0.25 in (0, 1] is accepted");
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
    fn mixed_kinds_register_all_supported() {
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(0.0, 0.0));
        sketch.add_circle(Point2d::new(1.0, 1.0), 0.5).expect("c");
        sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(2.0, 1.0))
            .expect("rect");
        sketch
            .add_ellipse(Point2d::new(3.0, 3.0), 2.0, 1.0, 0.0)
            .expect("ellipse");
        let _polyline_id = sketch
            .add_polyline(
                vec![
                    Point2d::new(0.0, 0.0),
                    Point2d::new(1.0, 0.0),
                    Point2d::new(1.0, 1.0),
                ],
                false,
            )
            .expect("polyline");

        let report = solve(&sketch).expect("solve");
        // 1 point + 1 circle + 1 rectangle + 1 ellipse + 1 polyline =
        // 5 supported; nothing skipped after slice C-5.
        assert_eq!(report.entities_solved, 5);
        assert_eq!(report.skipped_count(), 0);
        assert!(report.entities_skipped.is_empty());
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
        // Sanity check: every entity kind now lands as supported
        // (slice C-5 finished the matrix). One point + one arc + one
        // polyline = 3 supported, 0 skipped.
        let sketch = fresh_sketch();
        sketch.add_point(Point2d::new(0.0, 0.0));
        sketch
            .add_arc_center_angles(Point2d::new(0.0, 0.0), 1.0, 0.0, 1.0)
            .expect("arc");
        let _polyline_id = sketch
            .add_polyline(
                vec![
                    Point2d::new(2.0, 0.0),
                    Point2d::new(3.0, 0.0),
                    Point2d::new(3.0, 1.0),
                ],
                false,
            )
            .expect("polyline");
        let report = solve(&sketch).expect("solve");
        assert_eq!(report.entities_solved, 3);
        assert!(report.entities_skipped.is_empty());
    }

    // ── C-2: rectangle bridge ──────────────────────────────────────

    #[test]
    fn rectangle_geometry_round_trips_unchanged_with_no_constraints() {
        // Slice C-2: rectangles are first-class solver entities.
        // With no constraints the bridge must register the rectangle,
        // run the solver (which converges trivially), and write back
        // the identical parameters — preserving id, center, width,
        // height, and rotation.
        let sketch = fresh_sketch();
        let id = sketch
            .add_rectangle_rotated(
                Point2d::new(2.0, -1.0),
                3.0,
                1.5,
                std::f64::consts::PI / 6.0,
            )
            .expect("rect");

        let report = solve(&sketch).expect("solve");
        assert!(
            !report
                .entities_skipped
                .iter()
                .any(|e| matches!(e, EntityRef::Rectangle(_))),
            "rectangle must not appear in entities_skipped: {:?}",
            report.entities_skipped,
        );
        assert_eq!(report.entities_solved, 1);

        let entry = sketch.rectangles().get(&id).expect("rect survives");
        let r = &entry.value().rectangle;
        assert!((r.center.x - 2.0).abs() < 1e-10);
        assert!((r.center.y - (-1.0)).abs() < 1e-10);
        assert!((r.width - 3.0).abs() < 1e-10);
        assert!((r.height - 1.5).abs() < 1e-10);
        assert!((r.rotation - std::f64::consts::PI / 6.0).abs() < 1e-10);
    }

    #[test]
    fn rectangle_contributes_five_dofs_to_analysis() {
        // EntityState::rectangle parameter layout:
        // [center.x, center.y, width, height, rotation] → 5 DOFs.
        let sketch = fresh_sketch();
        sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(2.0, 1.0))
            .expect("rect");
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 5);
        assert_eq!(report.entities_analysed, 1);
        assert!(report.entities_skipped.is_empty());
    }

    #[test]
    fn rectangle_center_pinned_by_x_and_y_coordinate_dimensions() {
        // Drive the rectangle's center to (3, -2) using a pair of
        // dimensional constraints. EntityState::rectangle places
        // center at params[0..2] so the solver's generic
        // `get_point_position` path applies without dispatch
        // changes.
        let sketch = fresh_sketch();
        let id = sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(1.0, 1.0))
            .expect("rect");
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Rectangle(id)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(-2.0),
            vec![EntityRef::Rectangle(id)],
            ConstraintPriority::Required,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(report.converged(), "status was {:?}", report.status);

        let entry = sketch.rectangles().get(&id).expect("rect");
        let r = &entry.value().rectangle;
        assert!((r.center.x - 3.0).abs() < 1e-8);
        assert!((r.center.y - (-2.0)).abs() < 1e-8);
    }

    #[test]
    fn coincident_constraint_aligns_two_rectangle_centers() {
        // Pin one rectangle's center to (0, 0) via dimensional
        // constraints, then constrain a second rectangle to be
        // coincident with it. The bridge's `get_point_position`
        // helper treats rectangle params[0..2] as the centre, so
        // Coincident over two rectangles must collapse their
        // centres to the same point.
        let sketch = fresh_sketch();
        let pinned = sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(1.0, 1.0))
            .expect("a");
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(0.0),
            vec![EntityRef::Rectangle(pinned)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(0.0),
            vec![EntityRef::Rectangle(pinned)],
            ConstraintPriority::Required,
        ));
        let free = sketch
            .add_rectangle(Point2d::new(4.0, 5.0), Point2d::new(6.0, 7.0))
            .expect("b");
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![EntityRef::Rectangle(pinned), EntityRef::Rectangle(free)],
            ConstraintPriority::High,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(report.converged(), "status was {:?}", report.status);

        let entry = sketch.rectangles().get(&free).expect("rect");
        let r = &entry.value().rectangle;
        assert!(r.center.x.abs() < 1e-8, "center.x was {}", r.center.x);
        assert!(r.center.y.abs() < 1e-8, "center.y was {}", r.center.y);
    }

    #[test]
    fn equal_constraint_collapses_rectangle_dimensions() {
        // Equal(Rectangle, Rectangle) is a 2-residual constraint
        // (width AND height must match — rotation is independent).
        // Pin the first rectangle's dimensions via two distance-style
        // dimensional constraints would be circular; instead we
        // assert the converged solution agrees on (width, height)
        // without prescribing the absolute value.
        let sketch = fresh_sketch();
        let a = sketch
            .add_rectangle(Point2d::new(0.0, 0.0), Point2d::new(3.0, 2.0))
            .expect("a");
        let b = sketch
            .add_rectangle(Point2d::new(10.0, 10.0), Point2d::new(15.0, 12.0))
            .expect("b");
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Equal,
            vec![EntityRef::Rectangle(a), EntityRef::Rectangle(b)],
            ConstraintPriority::Required,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(report.converged(), "status was {:?}", report.status);

        let ra = sketch.rectangles().get(&a).expect("a");
        let rb = sketch.rectangles().get(&b).expect("b");
        let ra = &ra.value().rectangle;
        let rb = &rb.value().rectangle;
        assert!(
            (ra.width - rb.width).abs() < 1e-8,
            "widths diverged: {} vs {}",
            ra.width,
            rb.width,
        );
        assert!(
            (ra.height - rb.height).abs() < 1e-8,
            "heights diverged: {} vs {}",
            ra.height,
            rb.height,
        );
    }

    // ── C-3: ellipse bridge ───────────────────────────────────────

    #[test]
    fn ellipse_geometry_round_trips_unchanged_with_no_constraints() {
        // Slice C-3: ellipses are first-class solver entities. With
        // no constraints the bridge must register the ellipse, run
        // the solver (which converges trivially), and write back the
        // identical parameters — preserving id, centre, semi_major,
        // semi_minor, and rotation. The write-back goes through
        // `Ellipse2d::new`, which re-applies the `semi_major >=
        // semi_minor` invariant; we author the inputs in that order
        // so the round-trip is exact.
        let sketch = fresh_sketch();
        let id = sketch
            .add_ellipse(
                Point2d::new(2.0, -1.0),
                3.0,
                1.5,
                std::f64::consts::PI / 6.0,
            )
            .expect("ellipse");

        let report = solve(&sketch).expect("solve");
        assert!(
            !report
                .entities_skipped
                .iter()
                .any(|e| matches!(e, EntityRef::Ellipse(_))),
            "ellipse must not appear in entities_skipped: {:?}",
            report.entities_skipped,
        );
        assert_eq!(report.entities_solved, 1);

        let entry = sketch.ellipses().get(&id).expect("ellipse survives");
        let e = &entry.value().ellipse;
        assert!((e.center.x - 2.0).abs() < 1e-10);
        assert!((e.center.y - (-1.0)).abs() < 1e-10);
        assert!((e.semi_major - 3.0).abs() < 1e-10);
        assert!((e.semi_minor - 1.5).abs() < 1e-10);
        assert!((e.rotation - std::f64::consts::PI / 6.0).abs() < 1e-10);
    }

    #[test]
    fn ellipse_contributes_five_dofs_to_analysis() {
        // EntityState::ellipse parameter layout:
        // [center.x, center.y, semi_major, semi_minor, rotation] →
        // 5 DOFs. The bridge always reports 5 even for an axis-aligned
        // ellipse (where ParametricEllipse2d::degrees_of_freedom would
        // say 4) to avoid the DOF-badge flickering as the solver
        // nudges rotation across the tolerance threshold.
        let sketch = fresh_sketch();
        sketch
            .add_ellipse(Point2d::new(0.0, 0.0), 2.0, 1.0, 0.0)
            .expect("ellipse");
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 5);
        assert_eq!(report.entities_analysed, 1);
        assert!(report.entities_skipped.is_empty());
    }

    #[test]
    fn ellipse_center_pinned_by_x_and_y_coordinate_dimensions() {
        // Drive the ellipse's centre to (3, -2) using a pair of
        // dimensional constraints. EntityState::ellipse places centre
        // at params[0..2] so the solver's generic
        // `get_point_position` path applies without dispatch changes.
        let sketch = fresh_sketch();
        let id = sketch
            .add_ellipse(Point2d::new(0.0, 0.0), 2.0, 1.0, 0.0)
            .expect("ellipse");
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Ellipse(id)],
            ConstraintPriority::Required,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(-2.0),
            vec![EntityRef::Ellipse(id)],
            ConstraintPriority::Required,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(report.converged(), "status was {:?}", report.status);

        let entry = sketch.ellipses().get(&id).expect("ellipse");
        let e = &entry.value().ellipse;
        assert!((e.center.x - 3.0).abs() < 1e-8);
        assert!((e.center.y - (-2.0)).abs() < 1e-8);
    }

    #[test]
    fn equal_constraint_collapses_ellipse_axes() {
        // Equal(Ellipse, Ellipse) is a 2-residual constraint
        // (semi_major AND semi_minor must match — rotation is
        // independent). Assert the converged solution agrees on
        // (semi_major, semi_minor) without prescribing the absolute
        // value.
        let sketch = fresh_sketch();
        let a = sketch
            .add_ellipse(Point2d::new(0.0, 0.0), 3.0, 2.0, 0.0)
            .expect("a");
        let b = sketch
            .add_ellipse(Point2d::new(10.0, 10.0), 5.0, 1.0, 0.0)
            .expect("b");
        sketch.add_constraint(Constraint::new_geometric(
            GeometricConstraint::Equal,
            vec![EntityRef::Ellipse(a), EntityRef::Ellipse(b)],
            ConstraintPriority::Required,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(report.converged(), "status was {:?}", report.status);

        let ea = sketch.ellipses().get(&a).expect("a");
        let eb = sketch.ellipses().get(&b).expect("b");
        let ea = &ea.value().ellipse;
        let eb = &eb.value().ellipse;
        assert!(
            (ea.semi_major - eb.semi_major).abs() < 1e-8,
            "semi_majors diverged: {} vs {}",
            ea.semi_major,
            eb.semi_major,
        );
        assert!(
            (ea.semi_minor - eb.semi_minor).abs() < 1e-8,
            "semi_minors diverged: {} vs {}",
            ea.semi_minor,
            eb.semi_minor,
        );
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
        // Inject one violation so `converged()` correctly returns
        // false — an under-constrained sketch with a still-unsatisfied
        // residual is genuinely not solved. The empty-violations
        // case (= solver did satisfy every residual) is exercised
        // by `report_under_constrained_with_no_violations_is_solved`.
        let mut r = report_with_status(SolverStatus::UnderConstrained {
            degrees_of_freedom: 3,
        });
        r.violations.push((ConstraintId(uuid::Uuid::new_v4()), 1.0));
        assert!(!r.converged());
        assert!(r.is_under_constrained());
        assert!(!r.is_fully_constrained());
        assert_eq!(r.degrees_of_freedom(), Some(3));
        assert_eq!(r.iterations(), None);
        assert_eq!(r.final_error(), None);
        assert_eq!(r.conflicting_constraints(), None);
    }

    #[test]
    fn report_under_constrained_with_no_violations_is_solved() {
        // Tikhonov-regularised Newton can drive an under-constrained
        // system's residuals under tolerance — at which point
        // `converged()` reports true ("Solved" in Onshape parlance)
        // even though `is_fully_constrained()` stays false
        // (free DOFs remain).
        let r = report_with_status(SolverStatus::UnderConstrained {
            degrees_of_freedom: 3,
        });
        assert!(r.converged());
        assert!(r.is_under_constrained());
        assert!(!r.is_fully_constrained());
        assert_eq!(r.degrees_of_freedom(), Some(3));
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
    fn drag_constraints_synthesise_x_and_y_soft_for_point() {
        // Drag constraints are synthesised at `Low` priority so the
        // weighted-least-squares solver treats them as soft pulls —
        // any pre-existing `Required`/`High` constraint dominates,
        // and the dragged point lands on the closest reachable
        // location instead of forcibly snapping to an infeasible
        // cursor target.
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
            assert_eq!(c.priority, ConstraintPriority::Low);
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
    fn analyze_dofs_counts_polylines_as_supported() {
        // Slice C-5: polylines contribute 2 DOFs per vertex and are
        // surfaced as analysed (not skipped). A 3-vertex polyline
        // therefore contributes 6 free DOFs.
        let sketch = fresh_sketch();
        sketch
            .add_polyline(
                vec![
                    Point2d::new(0.0, 0.0),
                    Point2d::new(1.0, 0.0),
                    Point2d::new(1.0, 1.0),
                ],
                false,
            )
            .expect("polyline");
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 6);
        assert_eq!(report.entities_analysed, 1);
        assert!(report.entities_skipped.is_empty());
        assert_eq!(report.constraints_skipped, 0);
        assert!(!report.has_skipped_constraints());
    }

    // ── H: constraint diagnosis (redundancy + conflicts) ───────────

    #[test]
    fn analyze_dofs_fully_constrained_has_no_redundancy_or_conflicts() {
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
        assert!(report.is_fully_constrained());
        // Fully-constrained sketches skip the numerical pass.
        assert!(report.redundant.is_empty());
        assert!(report.conflicts.is_empty());
    }

    #[test]
    fn analyze_dofs_flags_redundant_duplicate_x_constraint() {
        // 1 free point + 3 XCoordinate(3.0): structurally
        // over-constrained (3 DOFs removed vs 2 free). Two of the
        // three X-constraints are linearly dependent on the first
        // (in sorted-by-id order) AND their residuals are zero →
        // both are redundant; the third is essential.
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(3.0, 0.0));
        let c1 = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        );
        let c2 = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        );
        let c3 = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        );
        let ids = [c1.id, c2.id, c3.id];
        sketch.add_constraint(c1);
        sketch.add_constraint(c2);
        sketch.add_constraint(c3);

        let report = analyze_dofs(&sketch);
        assert!(report.is_over_constrained());
        assert!(report.conflicts.is_empty(), "got {:?}", report.conflicts);
        // Exactly two of the three are redundant; the third (the one
        // with the smallest uuid by sort order) is the essential
        // representative — but we don't pin which from the outside.
        assert_eq!(report.redundant.len(), 2);
        for rid in &report.redundant {
            assert!(ids.contains(rid), "redundant id {rid:?} not in sketch");
        }
    }

    #[test]
    fn analyze_dofs_flags_conflicting_inconsistent_x_constraints() {
        // 1 free point at (3,0) + 3× XCoordinate(3.0/7.0/9.0).
        // Same Jacobian row for all three, different RHS — two of
        // them are linearly dependent on the essential one and their
        // residuals cannot all be zero. The two non-essential rows
        // must be classified as conflicts (not redundant).
        //
        // Diagnose runs only when the structural verdict is non-fully-
        // constrained, so we need ≥3 constraints on 2 DOFs to trigger
        // the over-constrained path.
        let sketch = fresh_sketch();
        let p = sketch.add_point(Point2d::new(3.0, 0.0));
        let c1 = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        );
        let c2 = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(7.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        );
        let c3 = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(9.0),
            vec![EntityRef::Point(p)],
            ConstraintPriority::Required,
        );
        let ids = [c1.id, c2.id, c3.id];
        sketch.add_constraint(c1);
        sketch.add_constraint(c2);
        sketch.add_constraint(c3);

        let report = analyze_dofs(&sketch);
        assert!(report.is_over_constrained());
        // Two of the three are dependent rows with non-zero residuals
        // → conflicts. The third (smallest uuid by sort order) is
        // essential. We don't pin which from the outside.
        assert_eq!(
            report.conflicts.len(),
            2,
            "expected 2 conflicts, got {:?}",
            report.conflicts
        );
        for cid in &report.conflicts {
            assert!(ids.contains(cid), "conflict id {cid:?} not in sketch");
        }
        assert!(report.redundant.is_empty(), "got {:?}", report.redundant);
    }

    #[test]
    fn analyze_dofs_under_constrained_sketch_has_no_conflicts() {
        // 2 free points + 1 distance: structurally
        // under-constrained (4 free DOFs - 1 removed = 3 free).
        // The lone distance row is independent → no redundancy,
        // no conflict.
        let sketch = fresh_sketch();
        let p1 = sketch.add_point(Point2d::new(0.0, 0.0));
        let p2 = sketch.add_point(Point2d::new(1.0, 0.0));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(1.0),
            vec![EntityRef::Point(p1), EntityRef::Point(p2)],
            ConstraintPriority::Required,
        ));
        let report = analyze_dofs(&sketch);
        assert!(report.is_under_constrained());
        assert!(report.redundant.is_empty());
        assert!(report.conflicts.is_empty());
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

    // ── C-4-b: spline registration + write-back ────────────────────

    /// Build the clamped-uniform B-spline used by the spline bridge
    /// tests: degree-2, four control points at (0,0), (1,2), (2,0),
    /// (3,2), open-uniform knot vector `[0,0,0,0.5,1,1,1]`. Mirrors
    /// the `sample_bspline_state` helper in `constraint_solver.rs`
    /// so the bridge-level tests exercise the same curve shape as
    /// the unit-level `point_on_bspline_*` tests.
    fn sample_bspline_inputs() -> (usize, Vec<Point2d>, Vec<f64>) {
        let control_points = vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 2.0),
            Point2d::new(2.0, 0.0),
            Point2d::new(3.0, 2.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        (2, control_points, knots)
    }

    #[test]
    fn bspline_registers_into_solver_not_skipped() {
        // C-4-b promotes splines from `entities_skipped` to
        // `entities_solved`. Empty constraint set → solve trivially
        // converges; the only assertion is that the spline made it
        // through the bridge.
        let sketch = fresh_sketch();
        let (degree, ctrl, knots) = sample_bspline_inputs();
        sketch
            .add_bspline(degree, ctrl, knots)
            .expect("add_bspline");
        let report = solve(&sketch).expect("solve");
        assert_eq!(report.entities_solved, 1);
        assert_eq!(report.skipped_count(), 0);
        assert!(report.entities_skipped.is_empty());
    }

    #[test]
    fn bspline_contributes_two_dofs_per_control_point() {
        // 4 CPs × 2 free coords = 8 DOFs (knots are pinned in
        // `SplineMetadata`, so only control-point coordinates move).
        // Matches `ParametricSpline2d::degrees_of_freedom` for the
        // non-rational case.
        let sketch = fresh_sketch();
        let (degree, ctrl, knots) = sample_bspline_inputs();
        sketch
            .add_bspline(degree, ctrl, knots)
            .expect("add_bspline");
        let report = analyze_dofs(&sketch);
        assert_eq!(report.total_free_dofs, 8);
        assert_eq!(report.entities_analysed, 1);
        assert!(report.entities_skipped.is_empty());
    }

    #[test]
    fn bspline_writeback_preserves_id_and_structural_metadata() {
        // Solve a sketch carrying just a B-spline (no constraints
        // means no parameter pull — but the write-back path still
        // runs on every solve via `apply_solver_result`). After the
        // solve, the spline id, degree, and knot vector must be
        // untouched; only control-point coordinates are permitted
        // to change, and even those should be at numerical-noise
        // distance from the originals when no constraint pulls on
        // them.
        let sketch = fresh_sketch();
        let (degree, ctrl, knots) = sample_bspline_inputs();
        let id = sketch
            .add_bspline(degree, ctrl.clone(), knots.clone())
            .expect("add_bspline");
        let _ = solve(&sketch).expect("solve");
        let entry = sketch.splines().get(&id).expect("spline survives id");
        match &entry.value().spline {
            Spline2d::BSpline(bs) => {
                assert_eq!(bs.degree, degree);
                assert_eq!(bs.knots, knots);
                assert_eq!(bs.control_points.len(), ctrl.len());
                for (got, want) in bs.control_points.iter().zip(ctrl.iter()) {
                    assert!((got.x - want.x).abs() < 1e-9);
                    assert!((got.y - want.y).abs() < 1e-9);
                }
            }
            Spline2d::Nurbs(_) => panic!("BSpline must round-trip as BSpline"),
        }
    }

    #[test]
    fn dcm_plate_squares_from_sloppy_input_with_shared_variables() {
        // SKETCH-DCM A.2 acceptance. Four deliberately sloppy corners,
        // four endpoint-linked lines, H/V on the lines plus two driving
        // distances on the points, one pinned corner. Pre-A.2 the lines
        // carried PRIVATE (point, direction) geometry, so Horizontal /
        // Vertical pulled on the lines' copies while Distance pulled on
        // the points: the live run reported under-constrained with ~16
        // phantom DOFs and the "solved" sketch was incoherent. With the
        // shared-variable model the line constraints differentiate
        // against the point variables and the plate squares exactly.
        let sketch = fresh_sketch();
        let p1 = sketch.add_point(Point2d::new(0.0, 0.0));
        let p2 = sketch.add_point(Point2d::new(82.0, 3.0));
        let p3 = sketch.add_point(Point2d::new(79.0, 48.0));
        let p4 = sketch.add_point(Point2d::new(-2.0, 51.0));
        sketch
            .points()
            .get_mut(&p1)
            .expect("p1 exists")
            .value_mut()
            .fix();
        let bottom = sketch.add_line(p1, p2).expect("bottom line");
        let right = sketch.add_line(p2, p3).expect("right line");
        let top = sketch.add_line(p3, p4).expect("top line");
        let left = sketch.add_line(p4, p1).expect("left line");

        for line in [bottom, top] {
            sketch.add_constraint(Constraint::new_geometric(
                GeometricConstraint::Horizontal,
                vec![EntityRef::Line(line)],
                ConstraintPriority::High,
            ));
        }
        for line in [right, left] {
            sketch.add_constraint(Constraint::new_geometric(
                GeometricConstraint::Vertical,
                vec![EntityRef::Line(line)],
                ConstraintPriority::High,
            ));
        }
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(80.0),
            vec![EntityRef::Point(p1), EntityRef::Point(p2)],
            ConstraintPriority::High,
        ));
        sketch.add_constraint(Constraint::new_dimensional(
            DimensionalConstraint::Distance(50.0),
            vec![EntityRef::Point(p1), EntityRef::Point(p4)],
            ConstraintPriority::High,
        ));

        let report = solve(&sketch).expect("solve");
        assert!(
            report.converged(),
            "plate must converge, got {:?}",
            report.status
        );

        let pos = |id| sketch.points().get(&id).expect("point").value().position;
        let tol = 1e-6;
        let (q1, q2, q3, q4) = (pos(p1), pos(p2), pos(p3), pos(p4));
        assert!(
            q1.x.abs() < tol && q1.y.abs() < tol,
            "p1 pinned, got {q1:?}"
        );
        assert!(
            (q2.x - 80.0).abs() < tol && q2.y.abs() < tol,
            "p2 must square to (80, 0), got {q2:?}"
        );
        assert!(
            (q3.x - 80.0).abs() < tol && (q3.y - 50.0).abs() < tol,
            "p3 must square to (80, 50), got {q3:?}"
        );
        assert!(
            (q4.x.abs() < tol) && (q4.y - 50.0).abs() < tol,
            "p4 must square to (0, 50), got {q4:?}"
        );

        // Coherence contract: every line's stored geometry must agree
        // EXACTLY with its endpoint points after the solve (the bridge
        // sync pass is the single writer of derived-segment geometry).
        for (line_id, a, b) in [
            (bottom, q1, q2),
            (right, q2, q3),
            (top, q3, q4),
            (left, q4, q1),
        ] {
            let entry = sketch.lines().get(&line_id).expect("line exists");
            let LineGeometry::Segment(seg) = entry.value().geometry else {
                panic!("expected segment geometry");
            };
            assert!(
                seg.start.distance_to(&a) < 1e-12 && seg.end.distance_to(&b) < 1e-12,
                "line {line_id:?} geometry detached from its points: {seg:?} vs ({a:?}, {b:?})"
            );
        }
    }
}
