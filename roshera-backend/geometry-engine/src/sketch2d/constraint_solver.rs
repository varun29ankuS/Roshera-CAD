//! Constraint solver for 2D sketches
//!
//! This module implements a geometric constraint solver using numerical methods.
//! The solver uses a combination of graph analysis and iterative numerical solving.
//!
//! # Algorithm
//!
//! 1. Build constraint graph
//! 2. Identify rigid clusters
//! 3. Order constraints by priority
//! 4. Solve using Newton-Raphson iteration
//! 5. Handle over/under-constrained cases
//!
//! Indexed access into Jacobian rows, residual vectors, and parameter arrays
//! is the canonical idiom for Newton-Raphson — all `arr[i]` sites are
//! bounds-guaranteed by the (n_params × n_constraints) system dimensions
//! established at solver entry. Matches the numerical-kernel pattern used in
//! nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::constraints::{ConstraintPriority, EntityRef};
use super::polyline2d::Polyline2d;
use super::spline2d::{BSpline2d, NurbsCurve2d};
use super::{
    Constraint, ConstraintId, ConstraintType, DimensionalConstraint, GeometricConstraint, Point2d,
    Tolerance2d, Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Solver status
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SolverStatus {
    /// Successfully solved all constraints
    Converged { iterations: usize, final_error: f64 },
    /// Failed to converge within iteration limit
    NotConverged { iterations: usize, final_error: f64 },
    /// System is over-constrained
    OverConstrained { conflicting_constraints: usize },
    /// System is under-constrained
    UnderConstrained { degrees_of_freedom: usize },
    /// Numerical instability detected
    Unstable,
}

/// Solver result
#[derive(Debug, Clone)]
pub struct SolverResult {
    /// Solver status
    pub status: SolverStatus,
    /// Updated entity positions
    pub entity_updates: HashMap<EntityRef, EntityUpdate>,
    /// Constraint violations
    pub violations: Vec<(ConstraintId, f64)>,
    /// Computation time in milliseconds
    pub solve_time_ms: f64,
}

/// Classification of every constraint in the solver after a numerical
/// diagnostic pass.
///
/// The Jacobian rows are processed in `constraints` order; each
/// constraint contributes one or more rows (see
/// `constraint_error_count`). A row is **independent** if its
/// component orthogonal to the span of previously processed rows
/// has norm above `STRICT_TOLERANCE`; otherwise it is **dependent**.
///
/// - A constraint is `redundant` when none of its rows are
///   independent **and** its post-solve residual is below the
///   solver's tolerance. The constraint duplicates information
///   already pinned by earlier constraints and can be safely
///   removed without changing the solution.
/// - A constraint is `conflicting` when none of its rows are
///   independent **and** its post-solve residual exceeds the
///   solver's tolerance. The constraint asks for something the
///   earlier (numerically prior) constraints make impossible — the
///   sketch is over-constrained at this row.
///
/// The classification is order-dependent: swapping the input
/// constraint order can shift which constraint of a redundant pair
/// is labelled redundant. The UI treats this as "this constraint
/// is part of a redundant/conflicting set"; pinpointing the global
/// minimum-cardinality MUS requires QuickXplain-style search
/// (deferred).
#[derive(Debug, Clone, Default)]
pub struct ConstraintDiagnosis {
    /// Constraints whose rows are linearly dependent on earlier rows
    /// and whose residual is within tolerance — safe to remove.
    pub redundant: Vec<ConstraintId>,
    /// Constraints whose rows are linearly dependent on earlier rows
    /// but whose residual exceeds tolerance — part of an
    /// inconsistent (over-constrained) subset.
    pub conflicts: Vec<ConstraintId>,
    /// Numerical rank of the Jacobian. `redundant.len() +
    /// conflicts.len() = num_rows - rank` (modulo duplicate rows
    /// from the same constraint).
    pub jacobian_rank: usize,
    /// Total number of residual rows analysed.
    pub jacobian_rows: usize,
}

/// Structural participation diagnostics for the most recent
/// [`ConstraintSolver::solve`] call (SKETCH-DCM #45 Slice 3).
///
/// The DR-plan contract requires drag re-solve scoping to be
/// observable *structurally* — "only the affected cluster chain
/// re-solves" must be assertable from counters, not inferred from
/// wall-clock. A "Newton run" here is any invocation of the damped
/// Newton core: the whole-system loop, one per-component loop, one
/// per-plan-step mini solve, or one cluster placement solve. A run
/// **participates** when it executes at least one iteration (a run
/// whose residual is already under tolerance exits before touching
/// the Jacobian and moves nothing).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SolveStats {
    /// Newton runs that executed ≥ 1 iteration.
    pub newton_runs_iterated: usize,
    /// Total free parameters across those runs — the participation
    /// measure. A solve that only had to move one 2-DOF point in a
    /// 100-parameter sketch reports 2 here, not 100.
    pub iterated_params: usize,
    /// Components solved through a completed DR-plan (Slice 3).
    pub planned_components: usize,
    /// Components solved by whole-component dense Newton — either
    /// because no complete DR-plan exists for them or because a plan
    /// failed verification and fell back.
    pub dense_components: usize,
    /// Plan executions that failed post-execution verification and
    /// were re-solved dense from the pre-plan state.
    pub plan_fallbacks: usize,
}

/// Entity position/parameter update
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params", rename_all = "snake_case")]
pub enum EntityUpdate {
    /// Updated point position
    Point(Point2d),
    /// Updated line parameters (point on line, direction)
    Line(Point2d, Vector2d),
    /// Updated arc parameters (center, radius, start_angle, end_angle)
    Arc(Point2d, f64, f64, f64),
    /// Updated circle parameters (center, radius)
    Circle(Point2d, f64),
    /// Updated rectangle parameters (center, width, height, rotation)
    Rectangle(Point2d, f64, f64, f64),
    /// Updated ellipse parameters (center, semi_major, semi_minor, rotation)
    Ellipse(Point2d, f64, f64, f64),
    /// Raw parameter vector for entities with variable degree of freedom
    /// (splines, polylines). Layout is entity-specific and matches the
    /// solver's internal `EntityState::parameters` order: for a spline,
    /// pairs of (x, y) per control point; for a polyline, pairs of (x, y)
    /// per vertex.
    Parameters(Vec<f64>),
}

/// Constraint solver
pub struct ConstraintSolver {
    /// Maximum iterations
    max_iterations: usize,
    /// Convergence tolerance
    tolerance: f64,
    /// Damping factor for Newton-Raphson
    damping_factor: f64,
    /// Entity positions/parameters
    entity_state: Arc<DashMap<EntityRef, EntityState>>,
    /// Active constraints
    constraints: Vec<Constraint>,
    /// Constraint dependencies
    dependency_graph: HashMap<ConstraintId, HashSet<EntityRef>>,
    /// SKETCH-DCM #45 Slice 2: when `true` (the default), `solve`
    /// splits the constraint graph into connected components and runs
    /// the Newton core per component (see [`Self::run_newton_decomposed`]).
    /// Disable via [`Self::set_decomposition_enabled`] to force the
    /// pre-slice one-big-system path — a diagnostics/benchmark switch
    /// (the `sketch_solver` bench uses it for its dense baseline), not
    /// a correctness knob: both paths must produce equivalent residual
    /// verdicts.
    decomposition_enabled: bool,
    /// SKETCH-DCM #45 Slice 3: when `true` (the default), each
    /// connected component is first offered to the rigid-cluster
    /// DR-planner (`dr_plan`); components the planner cannot fully
    /// decompose — and planned components that fail post-execution
    /// verification — solve through the existing whole-component
    /// Newton core exactly as before. Disable via
    /// [`Self::set_dr_plan_enabled`] for a Slice-2 baseline
    /// (component split without cluster planning).
    dr_plan_enabled: bool,
    /// Participation diagnostics for the most recent [`Self::solve`]
    /// call. See [`SolveStats`].
    last_stats: SolveStats,
}

/// Outcome of one run of the damped Newton-Raphson core (whole-system
/// or aggregated across components).
struct NewtonOutcome {
    /// Iterations executed. For the decomposed path this is the MAX
    /// over components — the depth of the longest Newton run, which is
    /// what the count means for latency (components are independent).
    iterations: usize,
    /// `‖residual‖₂` over the FULL constraint set at exit — measured
    /// identically in both paths.
    final_error: f64,
    /// True when any linear solve (any component) failed both the
    /// plain and Tikhonov-regularised attempts.
    linear_solve_failed: bool,
}

/// Outcome of offering one component to the DR-planner
/// (SKETCH-DCM #45 Slice 3). See [`ConstraintSolver::run_plan`].
enum PlanAttempt {
    /// No complete plan exists; entity states untouched.
    NoPlan,
    /// A plan executed but failed verification (or hit a numerical
    /// anomaly); entity states restored to the pre-plan snapshot.
    Fallback,
    /// Verified success; entity states hold the solution.
    Executed(PlanRunStats),
}

/// Participation tally of one executed DR-plan.
#[derive(Default)]
struct PlanRunStats {
    /// Steps whose Newton run executed ≥ 1 iteration.
    newton_runs_iterated: usize,
    /// Free parameters across those steps.
    iterated_params: usize,
    /// Max iterations over the plan's steps — the latency-relevant
    /// depth, mirroring `NewtonOutcome::iterations` semantics.
    max_iterations: usize,
}

/// One executed plan step's tally.
struct StepRun {
    iterations: usize,
    params: usize,
}

/// Structural metadata for a spline entity that cannot vary during
/// Newton–Raphson.
///
/// Knots, degree, and (when present) per-control-point weights are
/// pinned across solves: changing any of them is a structural edit
/// (knot refinement, degree elevation, rationality change) rather
/// than a constraint solve, and the solver has no continuous
/// gradient with respect to them. The control points themselves
/// live inside [`EntityState::parameters`] as a flat
/// `[cp0.x, cp0.y, cp1.x, cp1.y, …]` layout (2n entries for n control
/// points) regardless of whether the spline is rational.
///
/// Reconstruct the curve from a parameter vector + metadata pair via
/// [`ConstraintSolver::get_bspline2d`] (when `weights == None`) or
/// [`ConstraintSolver::get_nurbs_curve2d`] (when `weights == Some`).
#[derive(Debug, Clone)]
pub struct SplineMetadata {
    /// Polynomial degree of the spline (order = degree + 1).
    pub degree: usize,
    /// Knot vector (length = control_point_count + degree + 1).
    pub knots: Vec<f64>,
    /// Per-control-point weights. `None` = non-rational B-Spline;
    /// `Some(ws)` (with `ws.len() == control_point_count`) = NURBS.
    /// Weights are pinned because they alter the curve's rational
    /// denominator — varying them is a structural edit, not a
    /// constraint solve. If a user surface ever needs free-weight
    /// solving, the 3n parameter layout from the spline2d module doc
    /// is the natural extension and lands as a follow-up slice.
    pub weights: Option<Vec<f64>>,
}

/// Structural metadata for a polyline entity that cannot vary during
/// Newton–Raphson.
///
/// The `is_closed` flag pins the polyline's topology — flipping it
/// is a structural edit (adding or removing the wrap-around segment),
/// not a constraint solve. Vertex coordinates themselves live inside
/// [`EntityState::parameters`] as a flat `[v0.x, v0.y, v1.x, v1.y, …]`
/// layout (2n entries for n vertices).
///
/// Reconstruct the polyline from a parameter vector + metadata pair
/// via [`ConstraintSolver::get_polyline2d`].
#[derive(Debug, Clone)]
pub struct PolylineMetadata {
    /// Whether the polyline wraps the last vertex back to the first.
    /// Pinned across solves — a closed polyline stays closed.
    pub is_closed: bool,
}

/// Solver state for a single sketch entity.
///
/// Stores the entity's parameter vector together with a per-parameter
/// fixed-mask used by the Newton–Raphson solver. Construct one via
/// [`EntityState::point`], [`EntityState::line`], [`EntityState::circle`],
/// [`EntityState::arc`], [`EntityState::rectangle`],
/// [`EntityState::ellipse`], [`EntityState::spline_bspline`],
/// [`EntityState::spline_nurbs`], or [`EntityState::polyline`] and
/// register it with [`ConstraintSolver::add_entity`].
#[derive(Debug, Clone)]
pub struct EntityState {
    /// Current parameters (position, angles, etc.)
    parameters: Vec<f64>,
    /// Fixed parameters (indices that cannot change)
    fixed_mask: Vec<bool>,
    /// Structural metadata when this state describes a spline; `None`
    /// for every other entity kind. Carried inside `EntityState` so
    /// the solver can reconstruct a [`BSpline2d`] from the parameter
    /// vector without reaching back into the sketch.
    spline: Option<SplineMetadata>,
    /// Structural metadata when this state describes a polyline; `None`
    /// for every other entity kind. Carried inside `EntityState` so
    /// the solver can reconstruct a [`Polyline2d`] from the parameter
    /// vector without reaching back into the sketch. Mutually
    /// exclusive with [`Self::spline`] in practice (each constructor
    /// sets at most one), but the struct does not enforce that — the
    /// invariant is held by the constructor surface.
    polyline: Option<PolylineMetadata>,
    /// SHARED-VARIABLE MODEL (SKETCH-DCM A.2): when `Some((start, end))`,
    /// this entity is a segment line DERIVED from two point entities.
    /// It owns NO parameters of its own (`parameters` is empty, so the
    /// Jacobian sees zero columns from it); every geometric accessor
    /// (`get_line_point` / `get_line_direction` / `get_line_end` /
    /// `get_line_length`) computes from the endpoint points' live
    /// state. This is the D-Cubed-style coupling that makes a
    /// Horizontal constraint on a line and a Distance constraint on
    /// its endpoints pull on the SAME unknowns — previously lines
    /// carried a private (point, direction) copy, so solved sketches
    /// came apart (lines detached from their points) and DOF counts
    /// were inflated by 4 per line.
    derived_segment: Option<(EntityRef, EntityRef)>,
    /// SHARED-VARIABLE MODEL (SKETCH-DCM #45 Slice 1): when
    /// `Some((start, end))`, this entity is an ARC derived from two
    /// endpoint point entities. It owns exactly ONE parameter — see
    /// [`EntityState::arc_between`] for the parameterization and the
    /// rationale behind it. Every geometric accessor
    /// (`get_circle_center` / `get_circle_radius` / `get_arc_angles` /
    /// `get_point_position`) computes from the endpoint points' live
    /// state plus that parameter, so a `Radius` dimension on the arc
    /// and a `Distance` on its endpoints differentiate against the
    /// SAME unknowns — the line coupling above, extended to arcs.
    derived_arc_endpoints: Option<(EntityRef, EntityRef)>,
    /// SHARED-VARIABLE MODEL (SKETCH-DCM #45 Slice 1): when `Some`,
    /// this circle/arc entity's CENTER is the referenced point
    /// entity. The center coordinates are NOT in `parameters`
    /// (circle: `[radius]`; arc: `[radius, start_angle, end_angle]`)
    /// — they live in the point, once, so entities referencing the
    /// same point are concentric by construction and the DOF count
    /// cannot double-book the center. Mutually exclusive with
    /// [`Self::derived_arc_endpoints`]: the constructor surface only
    /// offers one at a time (honouring both simultaneously would need
    /// an implicit equidistance residual — refused rather than faked,
    /// see the `ParametricArc2d::center_point` doc).
    derived_center: Option<EntityRef>,
}

/// Decode the flat `[x0, y0, x1, y1, …]` spline-control-point pack
/// from an `EntityState::parameters` slice. Returns `None` if the
/// length is not even (the slice is malformed and the caller should
/// degrade to a zero residual rather than panic). Shared by
/// `get_bspline2d` and `get_nurbs_curve2d` because both share the
/// same 2n parameter layout — weights live in metadata, not in the
/// solved parameter vector.
fn control_points_from_parameters(parameters: &[f64]) -> Option<Vec<Point2d>> {
    if parameters.len() % 2 != 0 {
        return None;
    }
    let cp_count = parameters.len() / 2;
    let mut control_points = Vec::with_capacity(cp_count);
    for i in 0..cp_count {
        let base = i * 2;
        control_points.push(Point2d::new(parameters[base], parameters[base + 1]));
    }
    Some(control_points)
}

/// Normalize an angle to `[0, 2π)` — the convention `Arc2d` stores
/// its `start_angle` / `end_angle` in. Used when deriving arc angles
/// from shared endpoint points (`atan2` returns `(−π, π]`).
fn normalize_angle_2pi(angle: f64) -> f64 {
    let two_pi = 2.0 * std::f64::consts::PI;
    let normalized = angle.rem_euclid(two_pi);
    // rem_euclid can return exactly 2π for tiny negative inputs due
    // to rounding; fold it back to 0.
    if normalized >= two_pi {
        0.0
    } else {
        normalized
    }
}

/// Irreducible residual emitted for a constraint the solver recognises
/// but cannot yet enforce with a real equation (e.g. `MinDistance`,
/// `MomentOfInertia`, `MultiTangent`). It is a fixed, non-zero value with
/// an all-zero Jacobian row (the residual does not depend on any
/// parameter), so:
/// - the Newton step is unaffected (the row contributes nothing to
///   `Jᵀ·J` or `Jᵀ·e`), leaving the enforceable constraints to solve
///   normally, and
/// - the residual norm can never fall below tolerance, so the solver
///   reports `NotConverged`/violation rather than silently claiming a
///   solve for a constraint it is ignoring.
///
/// The magnitude (1.0) is deliberately above both the solver tolerance
/// and `CONFLICT_RESIDUAL_THRESHOLD` in the sketch bridge so the
/// unsupported constraint surfaces as a genuine conflict end-to-end.
/// Paired with `degrees_of_freedom_removed() == 0` for the same kinds
/// (see `constraints.rs`), the constraint neither fakes DOF accounting
/// nor fakes a satisfied residual — the self-cert lie is closed.
const UNSUPPORTED_CONSTRAINT_RESIDUAL: f64 = 1.0;

impl ConstraintSolver {
    /// Create a new constraint solver
    pub fn new() -> Self {
        Self {
            max_iterations: 100,
            tolerance: 1e-10,
            damping_factor: 0.5,
            entity_state: Arc::new(DashMap::new()),
            constraints: Vec::new(),
            dependency_graph: HashMap::new(),
            decomposition_enabled: true,
            dr_plan_enabled: true,
            last_stats: SolveStats::default(),
        }
    }

    /// Set maximum iterations
    pub fn set_max_iterations(&mut self, max_iterations: usize) {
        self.max_iterations = max_iterations;
    }

    /// Set convergence tolerance
    pub fn set_tolerance(&mut self, tolerance: f64) {
        self.tolerance = tolerance;
    }

    /// Set the Newton-Raphson damping factor.
    ///
    /// Each iteration applies `delta * damping_factor` to the entity
    /// parameters rather than the full Newton step. Values in `(0, 1]`
    /// are accepted; out-of-range inputs are clamped (≤ 0 maps to a
    /// tiny positive value, > 1 saturates at 1.0) so a misconfigured
    /// caller cannot stall the solver or drive it unstable. Callers
    /// that want strict validation should reject out-of-range inputs
    /// at their own layer (the sketch bridge in `sketch_solver.rs`
    /// does this).
    pub fn set_damping_factor(&mut self, damping_factor: f64) {
        self.damping_factor = damping_factor.clamp(f64::MIN_POSITIVE, 1.0);
    }

    /// Current damping factor (for diagnostics / tests).
    pub fn damping_factor(&self) -> f64 {
        self.damping_factor
    }

    /// Enable/disable connected-component decomposition (SKETCH-DCM
    /// #45 Slice 2). ON by default. Turning it off forces the
    /// pre-slice one-big-system Newton path — useful as a dense
    /// baseline for benchmarks and for mutation-testing the
    /// decomposition itself; residual verdicts must be equivalent
    /// either way (the gate pins this).
    pub fn set_decomposition_enabled(&mut self, enabled: bool) {
        self.decomposition_enabled = enabled;
    }

    /// Whether connected-component decomposition is active.
    pub fn decomposition_enabled(&self) -> bool {
        self.decomposition_enabled
    }

    /// Enable/disable the rigid-cluster DR-plan (SKETCH-DCM #45
    /// Slice 3). ON by default. Turning it off forces every component
    /// through the Slice-2 whole-component Newton path — a
    /// baseline/mutation-testing switch, not a correctness knob:
    /// residual verdicts must be equivalent either way. Has no effect
    /// while decomposition itself is disabled.
    pub fn set_dr_plan_enabled(&mut self, enabled: bool) {
        self.dr_plan_enabled = enabled;
    }

    /// Whether the rigid-cluster DR-plan is active.
    pub fn dr_plan_enabled(&self) -> bool {
        self.dr_plan_enabled
    }

    /// Participation diagnostics for the most recent [`Self::solve`]
    /// call. Zeroed at the start of every solve; a solver that has
    /// never solved reports all-zero stats.
    pub fn last_stats(&self) -> SolveStats {
        self.last_stats
    }

    /// Add an entity to the solver
    pub fn add_entity(&self, entity: EntityRef, initial_state: EntityState) {
        self.entity_state.insert(entity, initial_state);
    }

    /// Add constraints to solve
    pub fn set_constraints(&mut self, constraints: Vec<Constraint>) {
        self.constraints = constraints;
        self.build_dependency_graph();
    }

    /// Build constraint dependency graph
    fn build_dependency_graph(&mut self) {
        self.dependency_graph.clear();

        for constraint in &self.constraints {
            let entities: HashSet<EntityRef> = constraint.entities.iter().cloned().collect();
            self.dependency_graph.insert(constraint.id, entities);
        }
    }

    /// Solve the constraint system.
    ///
    /// Strategy: capture the DOF verdict from `check_constraint_count`
    /// but do **not** early-bail on over/under-constrained systems —
    /// Newton-Raphson still runs and `solve_linear_system` uses
    /// Tikhonov regularisation (J^T·J + λI) so a rank-deficient
    /// Jacobian still produces a well-defined minimum-norm step.
    /// This lets us drive a free point onto e.g. its required
    /// distance circle even when the system has a remaining rotational
    /// DOF, while still surfacing the DOF verdict to the caller.
    ///
    /// Final status precedence:
    /// 1. `Unstable` if linear solve never produced a step.
    /// 2. The captured DOF verdict (`OverConstrained` /
    ///    `UnderConstrained`) when present — even if Newton-Raphson
    ///    drove the residual under the tolerance, the system is still
    ///    structurally degenerate and the UI needs to know.
    /// 3. `Converged` / `NotConverged` otherwise.
    pub fn solve(&mut self) -> SolverResult {
        let start_time = std::time::Instant::now();
        self.last_stats = SolveStats::default();

        // Capture but do not act on the DOF verdict — it is folded
        // into the final status after Newton-Raphson runs. The count
        // is GLOBAL in both solve paths: the external verdict must be
        // indistinguishable whether or not decomposition ran.
        let dof_verdict = self.check_constraint_count();

        // Sort constraints by priority
        self.constraints.sort_by_key(|c| c.priority);

        // SKETCH-DCM #45 Slice 2: run the Newton core per connected
        // component of the constraint graph when there is more than
        // one. Components have block-diagonal Jacobians (zero coupling
        // by construction), so the per-component steps are the
        // whole-system steps restricted to each block — at
        // Σ O(pᵢ³) instead of O((Σpᵢ)³) per iteration.
        let outcome = self.run_newton_decomposed();

        // Determine final status with the precedence documented above.
        let status = if outcome.linear_solve_failed {
            SolverStatus::Unstable
        } else if let Some(verdict) = dof_verdict {
            verdict
        } else if outcome.final_error < self.tolerance {
            SolverStatus::Converged {
                iterations: outcome.iterations,
                final_error: outcome.final_error,
            }
        } else {
            SolverStatus::NotConverged {
                iterations: outcome.iterations,
                final_error: outcome.final_error,
            }
        };

        SolverResult {
            status,
            entity_updates: self.get_entity_updates(),
            violations: self.get_violations(),
            solve_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
        }
    }

    /// The damped Newton-Raphson core, byte-for-byte the pre-Slice-2
    /// `solve` loop. Operates on whatever entity/constraint set this
    /// solver instance holds; callers are responsible for having
    /// sorted `constraints` by priority.
    fn run_newton(&mut self) -> NewtonOutcome {
        let mut iteration = 0;
        let mut error = f64::INFINITY;
        let mut linear_solve_failed = false;

        while iteration < self.max_iterations && error > self.tolerance {
            // Compute constraint errors
            let errors = self.compute_constraint_errors();
            error = errors.iter().map(|e| e * e).sum::<f64>().sqrt();

            if error < self.tolerance {
                break;
            }

            // Compute Jacobian matrix
            let jacobian = self.compute_jacobian();

            // Empty Jacobian (no free parameters or no constraint
            // equations) — nothing to update; exit cleanly.
            if jacobian.is_empty() || jacobian[0].is_empty() {
                break;
            }

            // Solve linear system: J * dx = -errors
            match self.solve_linear_system(&jacobian, &errors) {
                Ok(delta) => {
                    // Apply updates with damping
                    self.apply_updates(&delta, self.damping_factor);
                }
                Err(_) => {
                    linear_solve_failed = true;
                    break;
                }
            }

            iteration += 1;
        }

        NewtonOutcome {
            iterations: iteration,
            final_error: error,
            linear_solve_failed,
        }
    }

    /// Dispatch the Newton core over connected components (SKETCH-DCM
    /// #45 Slice 2 — decomposition phase 0).
    ///
    /// A sketch that is one component (or a solver with decomposition
    /// disabled) runs the whole-system core unchanged — the split can
    /// only ever SHRINK the system Newton sees (spec §3.1 step 5).
    ///
    /// With k > 1 components, each component gets a sub-solver holding
    /// clones of exactly its entities and constraints, runs the same
    /// Newton core, and its solved parameters are written back. The
    /// aggregate is externally indistinguishable from the whole path:
    ///
    /// - `final_error` is re-measured GLOBALLY over the full constraint
    ///   set at exit — the same quantity the whole-system loop exits
    ///   with (constraints without any live entity included).
    /// - `linear_solve_failed` is true if ANY component's linear solve
    ///   failed, matching the whole path's any-iteration-failure ⇒
    ///   `Unstable` precedence.
    /// - `iterations` is the max over components (the longest chain —
    ///   see `NewtonOutcome::iterations`).
    ///
    /// Numerical equivalence note: component Jacobians are exact
    /// diagonal blocks of the whole-system Jacobian (cross-component
    /// entries are structurally zero), so per-component Newton steps
    /// equal the whole-system steps restricted to each block whenever
    /// the plain normal-equation solve succeeds. The one legitimate
    /// divergence is the Tikhonov fallback on rank-deficient systems:
    /// λ scales with the LOCAL trace instead of the joint one, so
    /// under-constrained components take their own well-scaled
    /// minimum-norm step instead of one polluted by (or polluting)
    /// unrelated components. Residual/verdict equivalence is what the
    /// gate pins, per the campaign contract.
    fn run_newton_decomposed(&mut self) -> NewtonOutcome {
        if !self.decomposition_enabled {
            return self.run_dense_whole();
        }
        let components = self.split_components();
        if components.len() <= 1 {
            // Single component (SKETCH-DCM #45 Slice 3): offer the
            // whole system to the DR-planner in place. `run_plan`
            // snapshots the entity states and restores them on any
            // miss, so the dense fallback below starts from EXACTLY
            // the pre-slice state — behaviour on unplannable sketches
            // is byte-identical to the Slice-2 path.
            if self.dr_plan_enabled {
                match self.run_plan() {
                    PlanAttempt::Executed(stats) => {
                        self.merge_plan_stats(&stats);
                        let errors = self.compute_constraint_errors();
                        let final_error = errors.iter().map(|e| e * e).sum::<f64>().sqrt();
                        return NewtonOutcome {
                            iterations: stats.max_iterations,
                            final_error,
                            linear_solve_failed: false,
                        };
                    }
                    PlanAttempt::Fallback => self.last_stats.plan_fallbacks += 1,
                    PlanAttempt::NoPlan => {}
                }
            }
            return self.run_dense_whole();
        }

        // The caller's tolerance bounds the GLOBAL residual norm.
        // k components that each stop at a local norm just under t
        // aggregate to a global norm up to √k·t, which would report
        // `NotConverged` for a solve the whole-system path calls
        // `Converged`. Tightening each component to t/√k makes the
        // aggregate provably meet the caller's tolerance:
        // ‖r‖ = √(Σ‖rᵢ‖²) < √(k·t²/k) = t.
        let active_components = components
            .iter()
            .filter(|component| !component.constraint_indices.is_empty())
            .count();
        let component_tolerance = if active_components > 1 {
            self.tolerance / (active_components as f64).sqrt()
        } else {
            self.tolerance
        };

        let mut iterations = 0usize;
        let mut linear_solve_failed = false;
        for component in &components {
            // Entities with no constraint rows anywhere take no Newton
            // step in either path (their columns are structurally
            // uninvolved) — skip the sub-solve entirely.
            if component.constraint_indices.is_empty() {
                continue;
            }
            let mut sub = ConstraintSolver::new();
            sub.max_iterations = self.max_iterations;
            sub.tolerance = component_tolerance;
            sub.damping_factor = self.damping_factor;
            sub.decomposition_enabled = false;
            for entity in &component.entities {
                if let Some(state) = self.entity_state.get(entity) {
                    sub.entity_state.insert(*entity, state.value().clone());
                }
            }
            sub.constraints = component
                .constraint_indices
                .iter()
                .filter_map(|&i| self.constraints.get(i).cloned())
                .collect();

            // SKETCH-DCM #45 Slice 3: offer the component to the
            // DR-planner first. On `Fallback`/`NoPlan` the sub-solver's
            // state is pristine (`run_plan` restores its snapshot), so
            // the dense path below is byte-identical to Slice 2.
            let mut planned = false;
            if self.dr_plan_enabled {
                match sub.run_plan() {
                    PlanAttempt::Executed(stats) => {
                        self.merge_plan_stats(&stats);
                        iterations = iterations.max(stats.max_iterations);
                        planned = true;
                    }
                    PlanAttempt::Fallback => self.last_stats.plan_fallbacks += 1,
                    PlanAttempt::NoPlan => {}
                }
            }
            if !planned {
                let sub_params = sub.count_degrees_of_freedom();
                let outcome = sub.run_newton();
                self.record_run(outcome.iterations, sub_params);
                self.last_stats.dense_components += 1;
                iterations = iterations.max(outcome.iterations);
                linear_solve_failed |= outcome.linear_solve_failed;
            }

            for entity in &component.entities {
                if let Some(state) = sub.entity_state.get(entity) {
                    self.entity_state.insert(*entity, state.value().clone());
                }
            }
        }

        // Exit residual measured globally over the merged state —
        // exactly the quantity the whole-system loop exits with.
        let errors = self.compute_constraint_errors();
        let final_error = errors.iter().map(|e| e * e).sum::<f64>().sqrt();
        NewtonOutcome {
            iterations,
            final_error,
            linear_solve_failed,
        }
    }

    /// Record one Newton run in [`Self::last_stats`]: a run
    /// participates only when it executed ≥ 1 iteration (see
    /// [`SolveStats`]).
    fn record_run(&mut self, iterations: usize, params: usize) {
        if iterations >= 1 {
            self.last_stats.newton_runs_iterated += 1;
            self.last_stats.iterated_params += params;
        }
    }

    /// Fold a planned component's execution stats into
    /// [`Self::last_stats`].
    fn merge_plan_stats(&mut self, stats: &PlanRunStats) {
        self.last_stats.newton_runs_iterated += stats.newton_runs_iterated;
        self.last_stats.iterated_params += stats.iterated_params;
        self.last_stats.planned_components += 1;
    }

    /// Whole-system dense Newton on `self` with stats accounting —
    /// the pre-Slice-3 path, byte-identical.
    fn run_dense_whole(&mut self) -> NewtonOutcome {
        let params = self.count_degrees_of_freedom();
        let outcome = self.run_newton();
        self.record_run(outcome.iterations, params);
        self.last_stats.dense_components += 1;
        outcome
    }

    // ── SKETCH-DCM #45 Slice 3: DR-plan execution ───────────────────
    //
    // Plan DISCOVERY lives in `dr_plan.rs` (pure, structural,
    // deterministic). Everything below is EXECUTION: running each
    // step as a small Newton solve against frozen placed geometry,
    // placing rigid clusters with an SE(2) transform solve, verifying
    // the achieved residuals, and restoring the pre-plan state on any
    // miss so the dense fallback reproduces pre-slice behaviour
    // exactly.

    /// Attempt DR-planned execution of this solver's system. The
    /// solver must already be scoped to ONE constraint-graph component
    /// (the whole system when it is a single component, or a Slice-2
    /// component sub-solver) with `constraints` priority-sorted.
    ///
    /// - [`PlanAttempt::NoPlan`]: no complete plan exists (entity
    ///   states untouched) — caller runs the dense core.
    /// - [`PlanAttempt::Fallback`]: a plan executed but failed
    ///   verification or hit a numerical anomaly; entity states have
    ///   been RESTORED to the pre-plan snapshot — caller runs the
    ///   dense core from exactly the state it would have seen before
    ///   this slice existed.
    /// - [`PlanAttempt::Executed`]: verified success; entity states
    ///   hold the solution.
    ///
    /// # Verification (the honesty gate)
    ///
    /// Plan discovery is generic-rigidity counting and can be lied to
    /// by degenerate geometry (spec §3.1: counting ⇒ *generically*
    /// rigid). Every planned solve is therefore verified: the
    /// unweighted L2 norm over all HARD (Required/High) enforced
    /// residuals must come in under the solver tolerance, widened by
    /// a soft-pull allowance when best-effort (Medium/Low) rows are
    /// present: at the weighted-least-squares stationary point a soft
    /// row legitimately displaces hard residuals by up to
    /// ~w²·|r_soft| (gradient norms are O(1) — coordinate rows are
    /// exactly unit), so demanding plain tolerance would spuriously
    /// fail every drag. The 10× factor is margin for non-unit
    /// gradient ratios; a configuration beyond it falls back to dense
    /// — slower, never wrong.
    fn run_plan(&mut self) -> PlanAttempt {
        // Already-satisfied components skip planning entirely: the
        // dense core exits at iteration 0 after ONE residual
        // evaluation — exactly what this check costs — so planning
        // here would only add discovery + per-step overhead to every
        // untouched component of an interactive drag frame (the
        // Slice-2 4–5 ms drag frame tripled before this early-out).
        let errors = self.compute_constraint_errors();
        let norm = errors.iter().map(|e| e * e).sum::<f64>().sqrt();
        if norm < self.tolerance {
            return PlanAttempt::NoPlan;
        }

        let Some((entities, constraints)) = self.extract_plan_inputs() else {
            return PlanAttempt::NoPlan;
        };
        let Some(plan) = super::dr_plan::plan_component(&entities, &constraints) else {
            return PlanAttempt::NoPlan;
        };
        if plan.steps.is_empty() {
            return PlanAttempt::NoPlan;
        }

        let snapshot: Vec<(EntityRef, EntityState)> = self
            .entity_state
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();

        // Each step stopping just under the component tolerance would
        // aggregate to √s·t over s steps; tighten per step so the
        // component-level norm provably meets `self.tolerance` — the
        // same argument as the Slice-2 per-component √k tightening.
        let step_tolerance = self.tolerance / (plan.steps.len() as f64).sqrt();

        let mut stats = PlanRunStats::default();
        let mut anomaly = false;
        for step in &plan.steps {
            let run = match step {
                super::dr_plan::PlanStep::Extend {
                    entity,
                    constraints: hard,
                    soft,
                } => self.execute_extend(*entity, hard, soft, step_tolerance),
                super::dr_plan::PlanStep::PlaceCluster {
                    entities: cluster,
                    internal,
                    boundary,
                    soft,
                } => self.execute_place_cluster(cluster, internal, boundary, soft, step_tolerance),
            };
            match run {
                Ok(step_run) => {
                    if step_run.iterations >= 1 {
                        stats.newton_runs_iterated += 1;
                        stats.iterated_params += step_run.params;
                    }
                    stats.max_iterations = stats.max_iterations.max(step_run.iterations);
                }
                Err(()) => {
                    anomaly = true;
                    break;
                }
            }
        }

        if !anomaly {
            let hard_norm = self.hard_enforced_residual_norm();
            let threshold = self.tolerance + 10.0 * self.soft_pull_allowance();
            if hard_norm < threshold {
                return PlanAttempt::Executed(stats);
            }
        }

        for (entity, state) in snapshot {
            self.entity_state.insert(entity, state);
        }
        PlanAttempt::Fallback
    }

    /// Build the abstract component model the planner consumes:
    /// entities with their free parameter counts, constraints with
    /// their variable sets expanded through the shared-variable model.
    /// Returns `None` when any constraint references an entity with no
    /// solver state (ghost refs) — the dense path's degrade-to-zero
    /// residual semantics must be preserved verbatim there.
    fn extract_plan_inputs(
        &self,
    ) -> Option<(
        Vec<super::dr_plan::PlanEntity>,
        Vec<super::dr_plan::PlanConstraint>,
    )> {
        use std::collections::BTreeSet;

        let mut entities = Vec::new();
        let mut free_map: HashMap<EntityRef, usize> = HashMap::new();
        let mut derived_map: HashMap<EntityRef, Vec<EntityRef>> = HashMap::new();
        for entry in self.entity_state.iter() {
            let entity = *entry.key();
            let state = entry.value();
            let free = state.free_param_count();
            free_map.insert(entity, free);
            derived_map.insert(entity, state.derived_refs());
            entities.push(super::dr_plan::PlanEntity {
                entity,
                free_dofs: free,
                // SE(2) cluster transforms are exact only for plain
                // 2-DOF points (their parameters ARE plane
                // coordinates). Everything else still takes Extend
                // steps, which need no transform.
                cluster_capable: matches!(entity, EntityRef::Point(_)) && free == 2,
            });
        }
        entities.sort_by_key(|pe| pe.entity);

        let mut constraints = Vec::with_capacity(self.constraints.len());
        for (index, c) in self.constraints.iter().enumerate() {
            let mut vars: BTreeSet<EntityRef> = BTreeSet::new();
            for raw in &c.entities {
                let free = *free_map.get(raw)?;
                if free > 0 {
                    vars.insert(*raw);
                }
                for derived in derived_map.get(raw).map(Vec::as_slice).unwrap_or(&[]) {
                    let dfree = *free_map.get(derived)?;
                    if dfree > 0 {
                        vars.insert(*derived);
                    }
                }
            }
            constraints.push(super::dr_plan::PlanConstraint {
                index,
                dof_removed: c.degrees_of_freedom_removed(),
                hard: super::dr_plan::is_hard_priority(c.priority),
                enforced: c.constraint_type.is_numerically_enforced(),
                grounded: super::dr_plan::references_frame(&c.constraint_type),
                vars: vars.into_iter().collect(),
            });
        }
        Some((entities, constraints))
    }

    /// Register everything a step's constraints can read into a step
    /// solver: `free_entities` keep their live state (and fixed
    /// masks); every other referenced entity — including derived-ref
    /// targets, which residual accessors reach through — is cloned
    /// FROZEN so the step cannot move placed geometry.
    fn register_step_entities(
        &self,
        step_solver: &ConstraintSolver,
        step_constraints: &[Constraint],
        free_entities: &[EntityRef],
    ) -> Result<(), ()> {
        let free_set: HashSet<EntityRef> = free_entities.iter().copied().collect();
        let mut queue: Vec<EntityRef> = free_entities.to_vec();
        for c in step_constraints {
            queue.extend(c.entities.iter().copied());
        }
        let mut seen: HashSet<EntityRef> = HashSet::new();
        while let Some(entity) = queue.pop() {
            if !seen.insert(entity) {
                continue;
            }
            let Some(state) = self.entity_state.get(&entity) else {
                return Err(());
            };
            queue.extend(state.value().derived_refs());
            let cloned = if free_set.contains(&entity) {
                state.value().clone()
            } else {
                state.value().frozen()
            };
            drop(state);
            step_solver.entity_state.insert(entity, cloned);
        }
        Ok(())
    }

    /// Build a step solver with this solver's tunables (decomposition
    /// and planning off — a step IS a leaf).
    fn step_solver(&self, step_tolerance: f64) -> ConstraintSolver {
        let mut mini = ConstraintSolver::new();
        mini.max_iterations = self.max_iterations;
        mini.tolerance = step_tolerance;
        mini.damping_factor = self.damping_factor;
        mini.decomposition_enabled = false;
        mini.dr_plan_enabled = false;
        mini
    }

    /// Clone the constraints at `indices`, priority-sorted (the
    /// `run_newton` contract).
    fn step_constraints(&self, indices: &[usize]) -> Result<Vec<Constraint>, ()> {
        let mut out = Vec::with_capacity(indices.len());
        for &i in indices {
            out.push(self.constraints.get(i).ok_or(())?.clone());
        }
        out.sort_by_key(|c| c.priority);
        Ok(out)
    }

    /// Execute an `Extend` step: solve `target`'s free parameters
    /// against its consumed constraints (plus soft passengers) with
    /// all placed geometry frozen, then adopt the solved state.
    fn execute_extend(
        &mut self,
        target: EntityRef,
        hard_indices: &[usize],
        soft_indices: &[usize],
        step_tolerance: f64,
    ) -> Result<StepRun, ()> {
        let mut mini = self.step_solver(step_tolerance);
        let all_indices: Vec<usize> = hard_indices
            .iter()
            .chain(soft_indices.iter())
            .copied()
            .collect();
        let constraints = self.step_constraints(&all_indices)?;
        self.register_step_entities(&mini, &constraints, &[target])?;
        mini.constraints = constraints;

        let params = mini.count_degrees_of_freedom();
        let outcome = mini.run_newton();
        if outcome.linear_solve_failed {
            return Err(());
        }
        let solved = mini.entity_state.get(&target).ok_or(())?.value().clone();
        self.entity_state.insert(target, solved);
        Ok(StepRun {
            iterations: outcome.iterations,
            params,
        })
    }

    /// Execute a `PlaceCluster` step.
    ///
    /// Phase 1 — internal shape solve: the cluster's own Newton run
    /// over its internal (shape) constraints. The system is
    /// rank-deficient by exactly the 3 rigid-body DOF, so the
    /// Tikhonov fallback in `solve_linear_system` yields the
    /// minimum-norm shape solution nearest the current state.
    ///
    /// Phase 2 — SE(2) placement: solve the 3 placement unknowns
    /// `(tx, ty, θ)` (rotation about the cluster centroid for
    /// conditioning) against the boundary constraints with a damped
    /// Newton on a finite-difference 3-column Jacobian, reusing
    /// `solve_linear_system` for the weighted normal equations. The
    /// rigid transform preserves the internally-solved shape exactly,
    /// which is the whole point of the decomposition: internal
    /// residuals cannot regress during placement.
    fn execute_place_cluster(
        &mut self,
        cluster: &[EntityRef],
        internal_indices: &[usize],
        boundary_indices: &[usize],
        soft_indices: &[usize],
        step_tolerance: f64,
    ) -> Result<StepRun, ()> {
        // Phase 1: internal shape.
        let mut internal_iterations = 0usize;
        let mut params = 0usize;
        if !internal_indices.is_empty() {
            let mut mini = self.step_solver(step_tolerance);
            let constraints = self.step_constraints(internal_indices)?;
            self.register_step_entities(&mini, &constraints, cluster)?;
            mini.constraints = constraints;
            params = mini.count_degrees_of_freedom();
            let outcome = mini.run_newton();
            if outcome.linear_solve_failed {
                return Err(());
            }
            internal_iterations = outcome.iterations;
            for entity in cluster {
                let solved = mini.entity_state.get(entity).ok_or(())?.value().clone();
                self.entity_state.insert(*entity, solved);
            }
        }

        // Phase 2: rigid placement.
        let scratch = self.step_solver(step_tolerance);
        let all_indices: Vec<usize> = boundary_indices
            .iter()
            .chain(soft_indices.iter())
            .copied()
            .collect();
        let constraints = self.step_constraints(&all_indices)?;
        self.register_step_entities(&scratch, &constraints, cluster)?;
        let mut scratch = scratch;
        scratch.constraints = constraints;

        // Base positions after the internal solve. Cluster members
        // are guaranteed plain 2-DOF points by the planner's
        // `cluster_capable` gate.
        let mut base: Vec<(EntityRef, f64, f64)> = Vec::with_capacity(cluster.len());
        for entity in cluster {
            let state = self.entity_state.get(entity).ok_or(())?;
            if state.value().parameters.len() < 2 {
                return Err(());
            }
            base.push((
                *entity,
                state.value().parameters[0],
                state.value().parameters[1],
            ));
        }
        if base.is_empty() {
            return Err(());
        }
        let cx = base.iter().map(|(_, x, _)| x).sum::<f64>() / base.len() as f64;
        let cy = base.iter().map(|(_, _, y)| y).sum::<f64>() / base.len() as f64;

        let apply = |scratch: &ConstraintSolver, u: &[f64; 3]| {
            let (sin_t, cos_t) = u[2].sin_cos();
            for (entity, bx, by) in &base {
                let dx = bx - cx;
                let dy = by - cy;
                if let Some(mut state) = scratch.entity_state.get_mut(entity) {
                    state.parameters[0] = cx + cos_t * dx - sin_t * dy + u[0];
                    state.parameters[1] = cy + sin_t * dx + cos_t * dy + u[1];
                }
            }
        };

        let mut u = [0.0f64; 3];
        let mut placement_iterations = 0usize;
        let h = 1e-8;
        while placement_iterations < self.max_iterations {
            apply(&scratch, &u);
            let errors = scratch.compute_constraint_errors();
            let norm = errors.iter().map(|e| e * e).sum::<f64>().sqrt();
            if norm < step_tolerance {
                break;
            }
            let mut jacobian = vec![vec![0.0; 3]; errors.len()];
            for k in 0..3 {
                let mut plus = u;
                plus[k] += h;
                apply(&scratch, &plus);
                let errors_plus = scratch.compute_constraint_errors();
                let mut minus = u;
                minus[k] -= h;
                apply(&scratch, &minus);
                let errors_minus = scratch.compute_constraint_errors();
                for (j, (ep, em)) in errors_plus.iter().zip(errors_minus.iter()).enumerate() {
                    jacobian[j][k] = (ep - em) / (2.0 * h);
                }
            }
            let delta = scratch.solve_linear_system(&jacobian, &errors)?;
            if delta.len() < 3 {
                return Err(());
            }
            u[0] += self.damping_factor * delta[0];
            u[1] += self.damping_factor * delta[1];
            u[2] += self.damping_factor * delta[2];
            placement_iterations += 1;
        }
        apply(&scratch, &u);

        // Adopt the placed positions, preserving each entity's own
        // fixed mask and derived metadata from the live state.
        for (entity, _, _) in &base {
            let placed = scratch.entity_state.get(entity).ok_or(())?;
            let (px, py) = (placed.value().parameters[0], placed.value().parameters[1]);
            drop(placed);
            let Some(mut live) = self.entity_state.get_mut(entity) else {
                return Err(());
            };
            if live.parameters.len() < 2 {
                return Err(());
            }
            live.parameters[0] = px;
            live.parameters[1] = py;
        }

        Ok(StepRun {
            iterations: internal_iterations.max(placement_iterations),
            params: params.max(3),
        })
    }

    /// Unweighted L2 norm over the residuals of every HARD
    /// (Required/High) numerically-enforced constraint — the plan
    /// verification quantity.
    fn hard_enforced_residual_norm(&self) -> f64 {
        self.constraints
            .iter()
            .filter(|c| {
                super::dr_plan::is_hard_priority(c.priority)
                    && c.constraint_type.is_numerically_enforced()
            })
            .flat_map(|c| self.evaluate_constraint_error(c))
            .map(|e| e * e)
            .sum::<f64>()
            .sqrt()
    }

    /// Legitimate hard-residual displacement allowance from soft
    /// (Medium/Low) rows: Σ w²·‖r_soft‖ over enforced soft
    /// constraints, evaluated at the current state. See `run_plan`.
    fn soft_pull_allowance(&self) -> f64 {
        self.constraints
            .iter()
            .filter(|c| {
                !super::dr_plan::is_hard_priority(c.priority)
                    && c.constraint_type.is_numerically_enforced()
            })
            .map(|c| {
                let w = priority_weight(c.priority);
                let errors = self.evaluate_constraint_error(c);
                w * w * errors.iter().map(|e| e * e).sum::<f64>().sqrt()
            })
            .sum()
    }

    /// Build the connected components of the current constraint graph.
    ///
    /// Nodes are the registered entities; edges are (a) every
    /// constraint's entity set and (b) the Slice-1 shared-variable
    /// references — a derived segment/arc is structurally coupled to
    /// its endpoint points, a shared-center circle/arc to its center
    /// point, so they must solve together even without an explicit
    /// constraint between them.
    fn split_components(&self) -> Vec<super::decompose::ConstraintComponent> {
        let mut nodes: Vec<EntityRef> = Vec::new();
        let mut shared_ref_edges: Vec<(EntityRef, EntityRef)> = Vec::new();
        for entry in self.entity_state.iter() {
            let owner = *entry.key();
            nodes.push(owner);
            let state = entry.value();
            if let Some((start, end)) = state.derived_segment {
                shared_ref_edges.push((owner, start));
                shared_ref_edges.push((owner, end));
            }
            if let Some((start, end)) = state.derived_arc_endpoints {
                shared_ref_edges.push((owner, start));
                shared_ref_edges.push((owner, end));
            }
            if let Some(center) = state.derived_center {
                shared_ref_edges.push((owner, center));
            }
        }
        let constraint_entities: Vec<&[EntityRef]> = self
            .constraints
            .iter()
            .map(|c| c.entities.as_slice())
            .collect();
        super::decompose::connected_components(&nodes, &shared_ref_edges, &constraint_entities)
    }

    /// Check if system is properly constrained
    fn check_constraint_count(&self) -> Option<SolverStatus> {
        let total_dof = self.count_degrees_of_freedom();
        let constraints_dof = self.count_constraint_dof();

        if constraints_dof > total_dof {
            Some(SolverStatus::OverConstrained {
                conflicting_constraints: constraints_dof - total_dof,
            })
        } else if constraints_dof < total_dof {
            Some(SolverStatus::UnderConstrained {
                degrees_of_freedom: total_dof - constraints_dof,
            })
        } else {
            None
        }
    }

    /// Count total degrees of freedom
    fn count_degrees_of_freedom(&self) -> usize {
        self.entity_state
            .iter()
            .map(|entry| {
                entry
                    .value()
                    .fixed_mask
                    .iter()
                    .filter(|&&fixed| !fixed)
                    .count()
            })
            .sum()
    }

    /// Count degrees of freedom removed by constraints
    fn count_constraint_dof(&self) -> usize {
        self.constraints
            .iter()
            .map(|c| c.degrees_of_freedom_removed())
            .sum()
    }

    /// Compute constraint errors (unweighted).
    ///
    /// The Newton-Raphson convergence check operates on these raw
    /// residuals — applying priority weights here would let the
    /// loop exit while the true geometric residual is still above
    /// tolerance, just because a low-priority constraint scaled the
    /// residual norm down. Priority weighting is applied inside
    /// [`solve_linear_system`] where it belongs (weighted least
    /// squares is a property of the linear solve, not of the
    /// residual measurement).
    fn compute_constraint_errors(&self) -> Vec<f64> {
        self.constraints
            .iter()
            .flat_map(|constraint| self.evaluate_constraint_error(constraint))
            .collect()
    }

    /// Per-residual priority weight vector, in the same row order as
    /// [`compute_constraint_errors`] / [`compute_jacobian`]. Each
    /// constraint contributes `constraint_error_count(c)` consecutive
    /// entries, all set to `priority_weight(c.priority)`. Used by
    /// [`solve_linear_system`] to assemble a weighted normal-equation
    /// matrix.
    fn priority_weights(&self) -> Vec<f64> {
        let mut weights = Vec::new();
        for c in &self.constraints {
            let w = priority_weight(c.priority);
            for _ in 0..self.constraint_error_count(c) {
                weights.push(w);
            }
        }
        weights
    }

    /// Evaluate error for a single constraint
    fn evaluate_constraint_error(&self, constraint: &Constraint) -> Vec<f64> {
        match &constraint.constraint_type {
            ConstraintType::Geometric(gc) => {
                self.evaluate_geometric_constraint(gc, &constraint.entities)
            }
            ConstraintType::Dimensional(dc) => {
                self.evaluate_dimensional_constraint(dc, &constraint.entities)
            }
        }
    }

    /// Evaluate geometric constraint error
    fn evaluate_geometric_constraint(
        &self,
        gc: &GeometricConstraint,
        entities: &[EntityRef],
    ) -> Vec<f64> {
        match gc {
            GeometricConstraint::Coincident => {
                // Two points should have same position
                if entities.len() == 2 {
                    if let (Some(p1), Some(p2)) = (
                        self.get_point_position(&entities[0]),
                        self.get_point_position(&entities[1]),
                    ) {
                        vec![p1.x - p2.x, p1.y - p2.y]
                    } else {
                        vec![0.0, 0.0]
                    }
                } else {
                    vec![0.0, 0.0]
                }
            }
            GeometricConstraint::Parallel => {
                // Two lines should have same direction
                if entities.len() == 2 {
                    if let (Some(d1), Some(d2)) = (
                        self.get_line_direction(&entities[0]),
                        self.get_line_direction(&entities[1]),
                    ) {
                        // Cross product should be zero
                        vec![d1.cross(&d2)]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Perpendicular => {
                // Two lines should be at 90 degrees
                if entities.len() == 2 {
                    if let (Some(d1), Some(d2)) = (
                        self.get_line_direction(&entities[0]),
                        self.get_line_direction(&entities[1]),
                    ) {
                        // Dot product should be zero
                        vec![d1.dot(&d2)]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Horizontal => {
                // Line should be horizontal (direction.y = 0)
                if entities.len() == 1 {
                    if let Some(dir) = self.get_line_direction(&entities[0]) {
                        vec![dir.y]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Vertical => {
                // Line should be vertical (direction.x = 0)
                if entities.len() == 1 {
                    if let Some(dir) = self.get_line_direction(&entities[0]) {
                        vec![dir.x]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Tangent => {
                // Line tangent to circle/arc
                if entities.len() == 2 {
                    self.evaluate_tangent_constraint(&entities[0], &entities[1])
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Concentric => {
                // Two circles/arcs share same center
                if entities.len() == 2 {
                    if let (Some(c1), Some(c2)) = (
                        self.get_circle_center(&entities[0]),
                        self.get_circle_center(&entities[1]),
                    ) {
                        vec![c1.x - c2.x, c1.y - c2.y]
                    } else {
                        vec![0.0, 0.0]
                    }
                } else {
                    vec![0.0, 0.0]
                }
            }
            GeometricConstraint::Equal => {
                // Two entities have equal dimension
                if entities.len() == 2 {
                    self.evaluate_equal_constraint(&entities[0], &entities[1])
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Symmetric => {
                // Entities symmetric about a line
                if entities.len() == 3 {
                    self.evaluate_symmetric_constraint(&entities[0], &entities[1], &entities[2])
                } else {
                    vec![0.0, 0.0]
                }
            }
            GeometricConstraint::PointOnCurve => {
                // Point lies on curve
                if entities.len() == 2 {
                    self.evaluate_point_on_curve(&entities[0], &entities[1])
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::Midpoint => {
                // Point at midpoint of line
                if entities.len() == 2 {
                    self.evaluate_midpoint_constraint(&entities[0], &entities[1])
                } else {
                    vec![0.0, 0.0]
                }
            }
            GeometricConstraint::Collinear => {
                // Three points are collinear
                if entities.len() == 3 {
                    self.evaluate_collinear_constraint(&entities[0], &entities[1], &entities[2])
                } else {
                    vec![0.0]
                }
            }
            GeometricConstraint::SmoothTangent => {
                // G1 continuity between curves
                if entities.len() == 2 {
                    self.evaluate_g1_continuity(&entities[0], &entities[1])
                } else {
                    vec![0.0, 0.0]
                }
            }
            GeometricConstraint::CurvatureContinuity => {
                // G2 continuity between curves
                if entities.len() == 2 {
                    self.evaluate_g2_continuity(&entities[0], &entities[1])
                } else {
                    vec![0.0, 0.0, 0.0]
                }
            }
            GeometricConstraint::IntersectionAngle(target_angle) => {
                // Entities are [line1, line2, intersection_point]; the
                // constrained quantity is the angle between the two line
                // directions (the point only locates where they meet, so it
                // does not enter the residual). See `angle_residual` for why
                // this is a single-valued vector residual rather than a
                // scalar cross/dot.
                if entities.len() >= 2 {
                    self.angle_residual(&entities[0], &entities[1], *target_angle)
                } else {
                    vec![0.0, 0.0]
                }
            }
            GeometricConstraint::EqualArea => {
                // Two closed entities enclose the same area: A₁ − A₂ = 0.
                if entities.len() == 2 {
                    match (
                        self.entity_area(&entities[0]),
                        self.entity_area(&entities[1]),
                    ) {
                        (Some(a1), Some(a2)) => vec![a1 - a2],
                        // An entity kind with no defined area here — refuse
                        // rather than fake a satisfied (zero) residual.
                        _ => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            GeometricConstraint::EqualPerimeter => {
                // Two closed entities have the same boundary length.
                if entities.len() == 2 {
                    match (
                        self.entity_perimeter(&entities[0]),
                        self.entity_perimeter(&entities[1]),
                    ) {
                        (Some(p1), Some(p2)) => vec![p1 - p2],
                        _ => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            GeometricConstraint::Centroid => {
                // entities = [point, closed_curve]; the point sits at the
                // curve's centre of mass. Two residuals (Δx, Δy) — matches
                // `constraint_error_count` and the 2 DOF it removes.
                if entities.len() == 2 {
                    if let (Some(p), Some(c)) = (
                        self.get_point_position(&entities[0]),
                        self.entity_centroid(&entities[1]),
                    ) {
                        vec![p.x - c.x, p.y - c.y]
                    } else {
                        // Keep the row budget (2) stable on failure.
                        vec![0.0, 0.0]
                    }
                } else {
                    vec![0.0, 0.0]
                }
            }
            // Recognised but not yet enforceable with a real equation.
            // Emit an irreducible residual so the solver refuses to claim
            // a solve rather than silently DOF-counting them (they also
            // remove 0 DOF — see `degrees_of_freedom_removed`).
            GeometricConstraint::Offset
            | GeometricConstraint::MultiTangent
            | GeometricConstraint::CurvatureExtremum
            | GeometricConstraint::ContactConstraint => Self::unsupported_residual(),
        }
    }

    /// Evaluate dimensional constraint error
    fn evaluate_dimensional_constraint(
        &self,
        dc: &DimensionalConstraint,
        entities: &[EntityRef],
    ) -> Vec<f64> {
        match dc {
            DimensionalConstraint::Distance(target_dist) => {
                // Distance between two points
                if entities.len() == 2 {
                    if let (Some(p1), Some(p2)) = (
                        self.get_point_position(&entities[0]),
                        self.get_point_position(&entities[1]),
                    ) {
                        let current_dist = p1.distance_to(&p2);
                        vec![current_dist - target_dist]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::Radius(target_radius) => {
                // Radius of circle or arc
                if entities.len() == 1 {
                    if let Some(radius) = self.get_circle_radius(&entities[0]) {
                        vec![radius - target_radius]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::XCoordinate(target_x) => {
                // X coordinate of point
                if entities.len() == 1 {
                    if let Some(pos) = self.get_point_position(&entities[0]) {
                        vec![pos.x - target_x]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::YCoordinate(target_y) => {
                // Y coordinate of point
                if entities.len() == 1 {
                    if let Some(pos) = self.get_point_position(&entities[0]) {
                        vec![pos.y - target_y]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::Angle(target_angle) => {
                // Fixed angle (radians) between two line directions.
                if entities.len() >= 2 {
                    self.angle_residual(&entities[0], &entities[1], *target_angle)
                } else {
                    vec![0.0, 0.0]
                }
            }
            DimensionalConstraint::Length(target_len) => {
                // Length of a line segment: |segment| − target.
                if entities.len() == 1 {
                    if let Some(len) = self.get_line_length(&entities[0]) {
                        vec![len - target_len]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::Diameter(target_dia) => {
                // Diameter of a circle/arc: 2·radius − target.
                if entities.len() == 1 {
                    if let Some(radius) = self.get_circle_radius(&entities[0]) {
                        vec![2.0 * radius - target_dia]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::Area(target_area) => {
                // Enclosed area of a closed entity: A − target.
                if entities.len() == 1 {
                    match self.entity_area(&entities[0]) {
                        Some(area) => vec![area - target_area],
                        None => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            DimensionalConstraint::Perimeter(target_perim) => {
                // Boundary length of a closed entity: P − target.
                if entities.len() == 1 {
                    match self.entity_perimeter(&entities[0]) {
                        Some(perim) => vec![perim - target_perim],
                        None => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            DimensionalConstraint::ArcLength(target_len) => {
                // Swept length of an arc/circle: L − target.
                if entities.len() == 1 {
                    match self.entity_arc_length(&entities[0]) {
                        Some(len) => vec![len - target_len],
                        None => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            DimensionalConstraint::Curvature(target_k) => {
                // Curvature at the entity: κ − target (κ = 1/r for
                // circle/arc, 0 for a line).
                if entities.len() == 1 {
                    match self.entity_curvature(&entities[0]) {
                        Some(k) => vec![k - target_k],
                        None => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            DimensionalConstraint::Slope(target_slope) => {
                // Slope of a line: dy/dx = target, written in the
                // division-free form dy − target·dx = 0 so the residual is
                // finite for vertical directions and drives the direction
                // to rotate rather than blow up.
                if entities.len() == 1 {
                    if let Some(dir) = self.get_line_direction(&entities[0]) {
                        vec![dir.y - target_slope * dir.x]
                    } else {
                        vec![0.0]
                    }
                } else {
                    vec![0.0]
                }
            }
            DimensionalConstraint::AspectRatio(target_ratio) => {
                // width/height = target for a rectangle, or
                // semi_major/semi_minor = target for an ellipse, written
                // division-free as dim0 − target·dim1 = 0.
                if entities.len() == 1 {
                    let dims = self
                        .get_rectangle_dimensions(&entities[0])
                        .or_else(|| self.get_ellipse_axes(&entities[0]));
                    match dims {
                        Some((major, minor)) => vec![major - target_ratio * minor],
                        None => Self::unsupported_residual(),
                    }
                } else {
                    Self::unsupported_residual()
                }
            }
            DimensionalConstraint::CenterOfMass { x, y } => {
                // Centre of mass pinned to (x, y). Two residuals (Δx, Δy)
                // — matches `constraint_error_count` and the 2 DOF removed.
                if entities.len() == 1 {
                    if let Some(c) = self.entity_centroid(&entities[0]) {
                        vec![c.x - x, c.y - y]
                    } else {
                        vec![0.0, 0.0]
                    }
                } else {
                    vec![0.0, 0.0]
                }
            }
            // Recognised but not yet enforceable with a real equation:
            // one-sided inequalities (Min/MaxDistance), moment of inertia,
            // and the ambiguous offset distance. Emit an irreducible
            // residual so the solver refuses to claim a solve — paired
            // with `degrees_of_freedom_removed() == 0` for these kinds.
            DimensionalConstraint::MinDistance(_)
            | DimensionalConstraint::MaxDistance(_)
            | DimensionalConstraint::MomentOfInertia(_)
            | DimensionalConstraint::OffsetDistance(_) => Self::unsupported_residual(),
        }
    }

    /// Get point position from entity state.
    ///
    /// For circles/arcs with a shared center or shared endpoints
    /// (SKETCH-DCM #45 Slice 1) the "point position" of the entity is
    /// its DERIVED center — preserving the legacy semantic where the
    /// leading two parameters (the center) served as the entity's
    /// point-like position for `Coincident` / `Distance` /
    /// `XCoordinate` / `YCoordinate`.
    fn get_point_position(&self, entity: &EntityRef) -> Option<Point2d> {
        let is_derived_center_like = self
            .entity_state
            .get(entity)
            .map(|s| s.derived_center.is_some() || s.derived_arc_endpoints.is_some());
        match is_derived_center_like {
            Some(true) => self.get_circle_center(entity),
            Some(false) => self.entity_state.get(entity).map(|state| {
                if state.parameters.len() >= 2 {
                    Point2d::new(state.parameters[0], state.parameters[1])
                } else {
                    Point2d::ORIGIN
                }
            }),
            None => None,
        }
    }

    /// Endpoint refs when `entity` is a derived segment line.
    fn derived_segment_of(&self, entity: &EntityRef) -> Option<(EntityRef, EntityRef)> {
        self.entity_state
            .get(entity)
            .and_then(|state| state.derived_segment)
    }

    /// Center point ref when `entity` carries a shared center
    /// (circle or arc, SKETCH-DCM #45 Slice 1).
    fn derived_center_of(&self, entity: &EntityRef) -> Option<EntityRef> {
        self.entity_state
            .get(entity)
            .and_then(|state| state.derived_center)
    }

    /// `(start, end, center_offset)` when `entity` is an
    /// endpoint-derived arc (SKETCH-DCM #45 Slice 1).
    fn derived_arc_endpoints_of(&self, entity: &EntityRef) -> Option<(EntityRef, EntityRef, f64)> {
        self.entity_state.get(entity).and_then(|state| {
            let (start, end) = state.derived_arc_endpoints?;
            let t = *state.parameters.first()?;
            Some((start, end, t))
        })
    }

    /// Full derived geometry `(center, radius, start_angle, end_angle)`
    /// of an endpoint-derived arc, computed from the shared endpoint
    /// points' live state and the arc's single chord-offset parameter
    /// (see [`EntityState::arc_between`]).
    ///
    /// Returns `None` when the entity is not an endpoint-derived arc,
    /// an endpoint state is missing, or the chord is degenerate
    /// (endpoints coincident within `STRICT_TOLERANCE`) — callers
    /// degrade the affected residual/update rather than emit NaN.
    fn derived_arc_geometry(&self, entity: &EntityRef) -> Option<(Point2d, f64, f64, f64)> {
        let (start, end, t) = self.derived_arc_endpoints_of(entity)?;
        let s = self.get_point_position(&start)?;
        let e = self.get_point_position(&end)?;
        let chord = Vector2d::new(e.x - s.x, e.y - s.y);
        let chord_len = chord.magnitude();
        if chord_len < STRICT_TOLERANCE.distance() {
            return None;
        }
        let u = Vector2d::new(chord.x / chord_len, chord.y / chord_len);
        // Left-hand perpendicular of the chord direction.
        let perp = Vector2d::new(-u.y, u.x);
        let mid = Point2d::new((s.x + e.x) / 2.0, (s.y + e.y) / 2.0);
        let center = Point2d::new(mid.x + t * perp.x, mid.y + t * perp.y);
        let half_chord = chord_len / 2.0;
        let radius = (t * t + half_chord * half_chord).sqrt();
        let start_angle = normalize_angle_2pi(f64::atan2(s.y - center.y, s.x - center.x));
        let end_angle = normalize_angle_2pi(f64::atan2(e.y - center.y, e.x - center.x));
        Some((center, radius, start_angle, end_angle))
    }

    /// Get line direction from entity state
    fn get_line_direction(&self, entity: &EntityRef) -> Option<Vector2d> {
        if let Some((start, end)) = self.derived_segment_of(entity) {
            let a = self.get_point_position(&start)?;
            let b = self.get_point_position(&end)?;
            return Some(Vector2d::new(b.x - a.x, b.y - a.y));
        }
        self.entity_state.get(entity).map(|state| {
            if state.parameters.len() >= 4 {
                // Parameters: point.x, point.y, dir.x, dir.y
                Vector2d::new(state.parameters[2], state.parameters[3])
            } else {
                Vector2d::UNIT_X
            }
        })
    }

    /// Single-valued angle residual between two line directions — the
    /// constraint "the signed rotation from `line1` to `line2` is
    /// `target_angle`".
    ///
    /// Form:
    /// ```text
    ///     r = d2_hat - R(θ)·d1_hat
    /// ```
    /// where `R(θ)` is rotation by `target_angle`. This is the single-valued
    /// vector form KittyCAD/ezpz adopted for `PointsAtAngle` (issue #244): a
    /// scalar residual built from the cross or dot product (`|d1 × d2|`,
    /// `sin(Δ − θ)`) vanishes at BOTH θ and θ+π, so a solve can slip to the
    /// antiparallel branch — the exact failure ezpz hit with its old
    /// `LinesAtAngle`. Comparing the full **unit** vectors makes the residual
    /// zero **only** when `d2` is `d1` rotated by exactly θ.
    ///
    /// ezpz scales their residual by `(|u| + |v|) / 2`, because their arms are
    /// point-to-point vectors whose lengths are real, pinned geometry — the
    /// prefactor then both scales the residual with the sketch and cancels the
    /// `1/|v|` from normalisation. We deliberately **do not** scale: our
    /// "arms" are line *direction* parameters with a free magnitude, so a
    /// magnitude prefactor would let the solver satisfy the constraint by
    /// collapsing a direction toward zero length instead of rotating it (the
    /// residual would shrink with `|d|` while the angle stayed wrong). The
    /// unit-difference form is invariant to `|d|`, so it cannot be cheated
    /// that way; because it is invariant, the magnitude column of the Jacobian
    /// is ~0 and the solver leaves `|d|` near its initial ~1, keeping the
    /// normalisation gradient bounded in practice.
    ///
    /// Returns two rows. They are rank 1 at the solution (both encode the one
    /// angular relation `∠d2 = ∠d1 + θ`), so the constraint removes exactly
    /// one DOF — consistent with `degrees_of_freedom_removed`. The
    /// rank-deficiency is absorbed by the Tikhonov fallback in
    /// [`solve_linear_system`].
    fn angle_residual(&self, line1: &EntityRef, line2: &EntityRef, target_angle: f64) -> Vec<f64> {
        let (Some(d1), Some(d2)) = (
            self.get_line_direction(line1),
            self.get_line_direction(line2),
        ) else {
            return vec![0.0, 0.0];
        };
        let n1 = d1.magnitude();
        let n2 = d2.magnitude();
        // A zero-length direction has no defined angle; emit no pull rather
        // than a NaN from the division.
        if n1 < STRICT_TOLERANCE.distance() || n2 < STRICT_TOLERANCE.distance() {
            return vec![0.0, 0.0];
        }
        let d1_hat = Vector2d::new(d1.x / n1, d1.y / n1);
        let d2_hat = Vector2d::new(d2.x / n2, d2.y / n2);
        let (sin_t, cos_t) = target_angle.sin_cos();
        // R(θ) · d1_hat
        let r_d1 = Vector2d::new(
            d1_hat.x * cos_t - d1_hat.y * sin_t,
            d1_hat.x * sin_t + d1_hat.y * cos_t,
        );
        vec![d2_hat.x - r_d1.x, d2_hat.y - r_d1.y]
    }

    /// Get circle radius from entity state
    fn get_circle_radius(&self, entity: &EntityRef) -> Option<f64> {
        match entity {
            EntityRef::Circle(_) | EntityRef::Arc(_) => {
                // Shared-variable dispatch (SKETCH-DCM #45 Slice 1):
                // an endpoint-derived arc's radius is a function of
                // its shared points + chord offset; a shared-center
                // entity stores the radius as its FIRST parameter
                // (circle `[r]`, arc `[r, a0, a1]`).
                if self.derived_arc_endpoints_of(entity).is_some() {
                    return self.derived_arc_geometry(entity).map(|(_, r, _, _)| r);
                }
                if self.derived_center_of(entity).is_some() {
                    return self
                        .entity_state
                        .get(entity)
                        .and_then(|state| state.parameters.first().copied());
                }
                self.entity_state.get(entity).map(|state| {
                    if state.parameters.len() >= 3 {
                        // Parameters: center.x, center.y, radius
                        state.parameters[2]
                    } else {
                        1.0
                    }
                })
            }
            _ => None,
        }
    }

    /// Get circle center from entity state.
    ///
    /// Also returns the centres of `Rectangle` and `Ellipse` entities
    /// since both store `[center.x, center.y, ...]` in the same
    /// leading two parameter slots. This lets `Concentric`
    /// constraints work between any pair of
    /// {Circle, Arc, Rectangle, Ellipse} without dispatching on
    /// entity-kind combinations.
    fn get_circle_center(&self, entity: &EntityRef) -> Option<Point2d> {
        match entity {
            EntityRef::Circle(_)
            | EntityRef::Arc(_)
            | EntityRef::Rectangle(_)
            | EntityRef::Ellipse(_) => {
                // Shared-variable dispatch (SKETCH-DCM #45 Slice 1):
                // a shared center IS the referenced point; an
                // endpoint-derived arc's center is computed from its
                // shared points + chord offset. Both terminate — the
                // referenced entity is a Point, which has neither
                // derived field.
                if let Some(center_ref) = self.derived_center_of(entity) {
                    return self.get_point_position(&center_ref);
                }
                if self.derived_arc_endpoints_of(entity).is_some() {
                    return self.derived_arc_geometry(entity).map(|(c, _, _, _)| c);
                }
                self.entity_state.get(entity).and_then(|state| {
                    if state.parameters.len() >= 2 {
                        // Parameters: center.x, center.y, ...
                        Some(Point2d::new(state.parameters[0], state.parameters[1]))
                    } else {
                        None
                    }
                })
            }
            _ => None,
        }
    }

    /// Get rectangle (width, height) from entity state.
    ///
    /// Returns `None` for non-rectangle entities. Used by the `Equal`
    /// constraint evaluator to compare both scalar dimensions when
    /// equating two rectangles.
    fn get_rectangle_dimensions(&self, entity: &EntityRef) -> Option<(f64, f64)> {
        match entity {
            EntityRef::Rectangle(_) => self.entity_state.get(entity).and_then(|state| {
                if state.parameters.len() >= 4 {
                    // Parameters: center.x, center.y, width, height, rotation
                    Some((state.parameters[2], state.parameters[3]))
                } else {
                    None
                }
            }),
            _ => None,
        }
    }

    /// Get ellipse `(semi_major, semi_minor)` from entity state.
    ///
    /// Returns `None` for non-ellipse entities. Used by the `Equal`
    /// constraint evaluator to compare both axis lengths when
    /// equating two ellipses; rotation is independent and excluded.
    fn get_ellipse_axes(&self, entity: &EntityRef) -> Option<(f64, f64)> {
        match entity {
            EntityRef::Ellipse(_) => self.entity_state.get(entity).and_then(|state| {
                if state.parameters.len() >= 4 {
                    // Parameters: center.x, center.y, semi_major, semi_minor, rotation
                    Some((state.parameters[2], state.parameters[3]))
                } else {
                    None
                }
            }),
            _ => None,
        }
    }

    /// Enclosed area of a closed entity.
    ///
    /// Defined for the closed primitive kinds the solver stores with a
    /// size parameter: circle (`πr²`), rectangle (`w·h`), ellipse
    /// (`π·a·b`). Returns `None` for open/ambiguous kinds (point, line,
    /// arc, spline, polyline) so callers refuse rather than fabricate a
    /// zero residual for an entity that has no well-defined area here.
    fn entity_area(&self, entity: &EntityRef) -> Option<f64> {
        use std::f64::consts::PI;
        match entity {
            EntityRef::Circle(_) => self.get_circle_radius(entity).map(|r| PI * r * r),
            EntityRef::Rectangle(_) => self.get_rectangle_dimensions(entity).map(|(w, h)| w * h),
            EntityRef::Ellipse(_) => self.get_ellipse_axes(entity).map(|(a, b)| PI * a * b),
            _ => None,
        }
    }

    /// Perimeter (closed-boundary length) of a closed entity.
    ///
    /// Circle: `2πr`. Rectangle: `2(w+h)`. Ellipse: Ramanujan's second
    /// approximation `π[3(a+b) − √((3a+b)(a+3b))]` (relative error
    /// < 1e-5 for all eccentricities — well inside sketch tolerance).
    /// `None` for kinds without a closed boundary the solver can size.
    fn entity_perimeter(&self, entity: &EntityRef) -> Option<f64> {
        use std::f64::consts::PI;
        match entity {
            EntityRef::Circle(_) => self.get_circle_radius(entity).map(|r| 2.0 * PI * r),
            EntityRef::Rectangle(_) => self
                .get_rectangle_dimensions(entity)
                .map(|(w, h)| 2.0 * (w + h)),
            EntityRef::Ellipse(_) => self
                .get_ellipse_axes(entity)
                .map(|(a, b)| PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt())),
            _ => None,
        }
    }

    /// Length of a curve entity along its swept parameter.
    ///
    /// Arc: `r·|θ_end − θ_start|`. Circle (closed): `2πr`. `None` for
    /// kinds whose swept length the solver cannot evaluate from its
    /// stored parameters.
    fn entity_arc_length(&self, entity: &EntityRef) -> Option<f64> {
        use std::f64::consts::PI;
        match entity {
            EntityRef::Arc(_) => {
                let r = self.get_circle_radius(entity)?;
                let (start, end) = self.get_arc_angles(entity)?;
                Some(r * (end - start).abs())
            }
            EntityRef::Circle(_) => self.get_circle_radius(entity).map(|r| 2.0 * PI * r),
            _ => None,
        }
    }

    /// Signed curvature magnitude of an entity: `1/r` for circle/arc,
    /// `0` for a straight line. `None` for kinds with non-constant or
    /// undefined curvature (handled elsewhere or refused).
    fn entity_curvature(&self, entity: &EntityRef) -> Option<f64> {
        match entity {
            EntityRef::Circle(_) | EntityRef::Arc(_) => {
                self.get_circle_radius(entity).map(|r| 1.0 / r)
            }
            EntityRef::Line(_) => Some(0.0),
            _ => None,
        }
    }

    /// Centre of mass of a closed primitive. For circle, arc, rectangle
    /// and ellipse this is the stored centre (`get_circle_center`
    /// already unifies those layouts). `None` for kinds whose centroid
    /// the solver cannot read directly.
    fn entity_centroid(&self, entity: &EntityRef) -> Option<Point2d> {
        self.get_circle_center(entity)
    }

    /// The refusal residual for a recognised-but-unenforceable
    /// constraint. See [`UNSUPPORTED_CONSTRAINT_RESIDUAL`].
    fn unsupported_residual() -> Vec<f64> {
        vec![UNSUPPORTED_CONSTRAINT_RESIDUAL]
    }

    /// Compute Jacobian matrix
    fn compute_jacobian(&self) -> Vec<Vec<f64>> {
        let num_errors = self
            .constraints
            .iter()
            .map(|c| self.constraint_error_count(c))
            .sum();
        let num_params = self.count_degrees_of_freedom();

        let mut jacobian = vec![vec![0.0; num_params]; num_errors];

        // Numerical differentiation for now
        let h = 1e-8;

        // Snapshot free-parameter descriptors so the DashMap iterator's read
        // guard is released before we hand mutating get_mut calls down to
        // perturb_parameter. Holding the iter() guard across get_mut on the
        // same shard would deadlock; this two-pass split is the safe pattern.
        let mut free_params: Vec<(EntityRef, usize, f64)> = Vec::new();
        for entry in self.entity_state.iter() {
            let entity = entry.key();
            let state = entry.value();
            for (i, &fixed) in state.fixed_mask.iter().enumerate() {
                if !fixed {
                    free_params.push((entity.clone(), i, state.parameters[i]));
                }
            }
        }

        for (param_index, (entity, i, original)) in free_params.into_iter().enumerate() {
            // Central difference
            self.perturb_parameter(&entity, i, original + h);
            let errors_plus = self.compute_constraint_errors();

            self.perturb_parameter(&entity, i, original - h);
            let errors_minus = self.compute_constraint_errors();

            // Restore original
            self.perturb_parameter(&entity, i, original);

            for (j, (ep, em)) in errors_plus.iter().zip(errors_minus.iter()).enumerate() {
                jacobian[j][param_index] = (ep - em) / (2.0 * h);
            }
        }

        jacobian
    }

    /// Perturb a parameter for numerical differentiation
    fn perturb_parameter(&self, entity: &EntityRef, param_index: usize, value: f64) {
        if let Some(mut state) = self.entity_state.get_mut(entity) {
            state.parameters[param_index] = value;
        }
    }

    /// Build the per-row constraint-id map.
    ///
    /// Entry `i` of the returned vector is the id of the constraint
    /// whose residual occupies row `i` of the Jacobian / error vector.
    /// Each constraint contributes `constraint_error_count(c)`
    /// consecutive entries. Used by [`ConstraintSolver::diagnose`] to
    /// translate row-level rank analysis back into constraint-level
    /// redundancy / conflict verdicts.
    pub fn jacobian_row_owners(&self) -> Vec<ConstraintId> {
        let mut owners = Vec::new();
        for c in &self.constraints {
            let n = self.constraint_error_count(c);
            for _ in 0..n {
                owners.push(c.id);
            }
        }
        owners
    }

    /// Classify the current constraint set into independent,
    /// redundant, and conflicting constraints.
    ///
    /// Procedure:
    /// 1. Build the Jacobian `J` (numerical, central difference) and
    ///    the row-owner map.
    /// 2. Run row-wise modified Gram-Schmidt with a rank-revealing
    ///    tolerance: a row whose component orthogonal to the span of
    ///    previously-accepted rows has norm < `STRICT_TOLERANCE *
    ///    max(||row||, 1.0)` is classified as **linearly dependent**.
    /// 3. Group the dependence verdict by constraint: a constraint is
    ///    "dependent" iff *every* row it owns was dependent.
    /// 4. Split dependent constraints by post-solve residual:
    ///    - residual ≤ `self.tolerance` → `redundant`
    ///    - residual >  `self.tolerance` → `conflicts`
    ///
    /// Does **not** mutate solver parameters; calls
    /// `compute_constraint_errors` after the Gram-Schmidt pass so the
    /// residual classification reflects the *current* parameter
    /// state. Callers that want the residuals to reflect a converged
    /// solution should run `solve()` before `diagnose()`.
    pub fn diagnose(&self) -> ConstraintDiagnosis {
        let jacobian = self.compute_jacobian();
        let row_owners = self.jacobian_row_owners();
        let residuals = self.compute_constraint_errors();
        let jacobian_rows = jacobian.len();
        if jacobian_rows == 0 || row_owners.is_empty() {
            return ConstraintDiagnosis {
                redundant: Vec::new(),
                conflicts: Vec::new(),
                jacobian_rank: 0,
                jacobian_rows,
            };
        }

        // Modified Gram-Schmidt rank analysis. `basis` holds the
        // accepted (orthonormal) row vectors; `row_independent[i]`
        // tracks whether row `i` contributed to the basis.
        let n_cols = jacobian[0].len();
        let mut basis: Vec<Vec<f64>> = Vec::new();
        let mut row_independent = vec![false; jacobian_rows];

        for (i, row) in jacobian.iter().enumerate() {
            if row.len() != n_cols {
                // Defensive: jagged rows shouldn't happen because
                // `compute_jacobian` allocates a uniform matrix, but
                // we degrade gracefully rather than panic.
                continue;
            }
            // Project out the span of the existing basis.
            let mut residual = row.clone();
            for b in &basis {
                let dot: f64 = residual.iter().zip(b.iter()).map(|(r, v)| r * v).sum();
                for (rk, vk) in residual.iter_mut().zip(b.iter()) {
                    *rk -= dot * vk;
                }
            }
            let norm: f64 = residual.iter().map(|x| x * x).sum::<f64>().sqrt();
            let row_norm: f64 = row.iter().map(|x| x * x).sum::<f64>().sqrt();
            let scale = row_norm.max(1.0);
            if norm > STRICT_TOLERANCE.distance() * scale {
                // Independent — normalise and add to the basis.
                for rk in residual.iter_mut() {
                    *rk /= norm;
                }
                basis.push(residual);
                row_independent[i] = true;
            }
        }

        let jacobian_rank = basis.len();

        // Group by owning constraint id, preserving first-seen order
        // so the returned vectors are deterministic for any given
        // constraint ordering.
        let mut order: Vec<ConstraintId> = Vec::new();
        let mut all_dependent: HashMap<ConstraintId, bool> = HashMap::new();
        let mut residual_sq: HashMap<ConstraintId, f64> = HashMap::new();
        for (i, &owner) in row_owners.iter().enumerate() {
            if !all_dependent.contains_key(&owner) {
                order.push(owner);
                all_dependent.insert(owner, true);
                residual_sq.insert(owner, 0.0);
            }
            if row_independent[i] {
                if let Some(v) = all_dependent.get_mut(&owner) {
                    *v = false;
                }
            }
            if let Some(r) = residuals.get(i) {
                if let Some(acc) = residual_sq.get_mut(&owner) {
                    *acc += r * r;
                }
            }
        }

        let mut redundant = Vec::new();
        let mut conflicts = Vec::new();
        for cid in order {
            if all_dependent.get(&cid).copied().unwrap_or(false) {
                let mag = residual_sq.get(&cid).copied().unwrap_or(0.0).sqrt();
                if mag <= self.tolerance {
                    redundant.push(cid);
                } else {
                    conflicts.push(cid);
                }
            }
        }

        ConstraintDiagnosis {
            redundant,
            conflicts,
            jacobian_rank,
            jacobian_rows,
        }
    }

    /// Count error components for a constraint.
    ///
    /// Most constraints contribute a fixed number of residual rows
    /// driven only by their constraint variant. A small number depend
    /// on the entity kinds in `constraint.entities`:
    /// - `Equal` between two rectangles produces 2 residuals (width
    ///   diff + height diff); `Equal` between two ellipses produces
    ///   2 residuals (semi_major diff + semi_minor diff); every other
    ///   `Equal` pair produces 1.
    fn constraint_error_count(&self, constraint: &Constraint) -> usize {
        match &constraint.constraint_type {
            ConstraintType::Geometric(gc) => match gc {
                GeometricConstraint::Coincident => 2,
                GeometricConstraint::Parallel => 1,
                GeometricConstraint::Perpendicular => 1,
                GeometricConstraint::Horizontal => 1,
                GeometricConstraint::Vertical => 1,
                GeometricConstraint::Equal => {
                    if constraint.entities.len() == 2
                        && ((matches!(constraint.entities[0], EntityRef::Rectangle(_))
                            && matches!(constraint.entities[1], EntityRef::Rectangle(_)))
                            || (matches!(constraint.entities[0], EntityRef::Ellipse(_))
                                && matches!(constraint.entities[1], EntityRef::Ellipse(_))))
                    {
                        2
                    } else {
                        1
                    }
                }
                // The angle residual (`angle_residual`) is a 2-vector.
                GeometricConstraint::IntersectionAngle(_) => 2,
                // Centroid pins a point to a centre of mass: (Δx, Δy).
                GeometricConstraint::Centroid => 2,
                _ => 1,
            },
            // `DimensionalConstraint::Angle` shares the 2-vector angle
            // residual; `CenterOfMass` pins (x, y). Every other
            // dimensional constraint is scalar.
            ConstraintType::Dimensional(DimensionalConstraint::Angle(_))
            | ConstraintType::Dimensional(DimensionalConstraint::CenterOfMass { .. }) => 2,
            ConstraintType::Dimensional(_) => 1,
        }
    }

    /// Solve the normal-equations linear system used by Newton-Raphson:
    /// `(J^T·J) · dx = -J^T · errors`. When `J^T·J` is singular —
    /// which happens whenever the Jacobian is rank-deficient
    /// (under-constrained systems, redundant constraints, parallel
    /// constraint gradients) — Gaussian elimination fails. We then
    /// fall back to **Tikhonov regularisation**: add `λI` to the
    /// diagonal and re-solve. This gives the minimum-norm
    /// least-squares step, which is exactly the right behaviour for
    /// under-constrained sketches (the solver advances the
    /// parameters by the smallest amount that satisfies the active
    /// constraints, leaving the residual DOFs untouched).
    fn solve_linear_system(&self, jacobian: &[Vec<f64>], errors: &[f64]) -> Result<Vec<f64>, ()> {
        let n = jacobian[0].len();
        let m = jacobian.len();

        if m == 0 || n == 0 {
            return Ok(vec![0.0; n]);
        }

        // Apply priority weights at the linear-solve layer: each
        // row k of J and each entry k of errors carries a weight
        // w_k = priority_weight(constraint_k.priority). The
        // weighted normal equations are
        //     (W·J)^T · (W·J) · dx = -(W·J)^T · (W·errors)
        //   = (J^T · W² · J) · dx = -J^T · W² · errors
        // so the entries of `J^T·J` and `J^T·errors` accumulate
        // with a `w² = w_k * w_k` factor. The convergence loop sees
        // unweighted residuals; weighting only biases the Newton
        // *step direction* toward satisfying higher-priority
        // constraints first.
        let weights = self.priority_weights();
        // Defensive: if priority_weights() yields fewer entries
        // than rows (should not happen — it iterates the same
        // constraints) fall back to a weight of 1.0 so we still
        // produce a meaningful step instead of indexing out of
        // bounds.
        let w_at = |k: usize| weights.get(k).copied().unwrap_or(1.0);

        // SKETCH-DCM #45 Slice 1 (drag honesty): restrict the normal
        // equations to the columns actually touched by at least one
        // residual row. A parameter with an all-zero Jacobian column
        // is structurally uninvolved in this iteration and its
        // minimum-norm step is exactly 0 — solving for it adds
        // nothing but singularity. Previously the FULL n×n system was
        // assembled, so J^T·J was singular whenever ANY such column
        // existed, and the trace-scaled Tikhonov λ (trace/n · 1e-8)
        // could then land below the Gaussian pivot threshold (1e-9) —
        // e.g. a Low-priority drag (weight 1e-3, squared to 1e-6 in
        // the trace) in a sketch containing any other unconstrained
        // entity — making the fallback fail too: the solver reported
        // `Unstable` and the dragged point never moved. Reducing
        // first keeps the solved block at its natural scale and
        // leaves untouched parameters untouched by construction.
        // (Central differences produce EXACT 0.0 entries for
        // uninvolved parameters, so the `!= 0.0` test is not a
        // tolerance judgement.)
        let involved: Vec<usize> = (0..n)
            .filter(|&col| jacobian.iter().any(|row| row[col] != 0.0))
            .collect();
        if involved.is_empty() {
            return Ok(vec![0.0; n]);
        }
        let nr = involved.len();

        // Compute J^T * W² * J over the involved columns.
        let mut jtj = vec![vec![0.0; nr]; nr];
        for (ri, &ci) in involved.iter().enumerate() {
            for (rj, &cj) in involved.iter().enumerate() {
                for k in 0..m {
                    let w2 = w_at(k) * w_at(k);
                    jtj[ri][rj] += w2 * jacobian[k][ci] * jacobian[k][cj];
                }
            }
        }

        // Compute -J^T * W² * errors over the involved columns.
        let mut jte = vec![0.0; nr];
        for (ri, &ci) in involved.iter().enumerate() {
            for j in 0..m {
                let w2 = w_at(j) * w_at(j);
                jte[ri] -= w2 * jacobian[j][ci] * errors[j];
            }
        }

        // First attempt: plain Gaussian elimination on J^T·J.
        // For well- and over-determined systems this is numerically
        // ideal — λ would only introduce shrinkage bias.
        let reduced = if let Ok(x) = self.gaussian_elimination(jtj.clone(), jte.clone()) {
            x
        } else {
            // Fallback: Tikhonov-regularised solve.
            // λ is scaled by the trace of J^T·J so it adapts to the
            // problem magnitude — a constant λ would over-regularise
            // small-residual systems and under-regularise large ones.
            let trace: f64 = (0..nr).map(|i| jtj[i][i]).sum();
            let lambda = if trace > 0.0 {
                (trace / nr as f64) * 1e-8
            } else {
                STRICT_TOLERANCE.distance()
            };
            for (i, row) in jtj.iter_mut().enumerate() {
                row[i] += lambda;
            }
            self.gaussian_elimination(jtj, jte)?
        };

        // Scatter back: uninvolved columns take the exact
        // minimum-norm step of 0.
        let mut full = vec![0.0; n];
        for (ri, &ci) in involved.iter().enumerate() {
            full[ci] = reduced[ri];
        }
        Ok(full)
    }

    /// Gaussian elimination solver
    fn gaussian_elimination(&self, mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Result<Vec<f64>, ()> {
        let n = a.len();

        // Forward elimination
        for k in 0..n {
            // Find pivot
            let mut max_row = k;
            for i in (k + 1)..n {
                if a[i][k].abs() > a[max_row][k].abs() {
                    max_row = i;
                }
            }

            // Swap rows
            a.swap(k, max_row);
            b.swap(k, max_row);

            // Check for singular matrix
            if a[k][k].abs() < STRICT_TOLERANCE.distance() {
                return Err(());
            }

            // Eliminate below pivot
            for i in (k + 1)..n {
                let factor = a[i][k] / a[k][k];
                for j in k..n {
                    a[i][j] -= factor * a[k][j];
                }
                b[i] -= factor * b[k];
            }
        }

        // Back substitution
        let mut x = vec![0.0; n];
        for i in (0..n).rev() {
            x[i] = b[i];
            for j in (i + 1)..n {
                x[i] -= a[i][j] * x[j];
            }
            x[i] /= a[i][i];
        }

        Ok(x)
    }

    /// Apply parameter updates
    fn apply_updates(&self, delta: &[f64], damping: f64) {
        let mut param_index = 0;
        let mut updates = Vec::new();

        // Collect updates first
        for entry in self.entity_state.iter() {
            let entity = *entry.key();
            let mut state = entry.value().clone();

            for (i, &fixed) in state.fixed_mask.iter().enumerate() {
                if !fixed {
                    state.parameters[i] += damping * delta[param_index];
                    param_index += 1;
                }
            }

            updates.push((entity, state));
        }

        // Apply updates
        for (entity, state) in updates {
            self.entity_state.insert(entity, state);
        }
    }

    /// Get entity updates for result
    fn get_entity_updates(&self) -> HashMap<EntityRef, EntityUpdate> {
        let mut updates = HashMap::new();
        // Circles/arcs with shared refs (SKETCH-DCM #45 Slice 1) need
        // accessor calls that re-enter the entity map (to read the
        // shared points). Collect them during the iteration and
        // compute their updates AFTER the iterator guard is released
        // — never take a nested shard read while holding it.
        let mut derived_curves: Vec<EntityRef> = Vec::new();

        for entry in self.entity_state.iter() {
            let entity = *entry.key();
            let state = entry.value();

            let update = match entity {
                EntityRef::Point(_) => {
                    EntityUpdate::Point(Point2d::new(state.parameters[0], state.parameters[1]))
                }
                EntityRef::Line(_) => {
                    if state.derived_segment.is_some() {
                        // Derived segments own no parameters; their
                        // geometry is synced from the endpoint points
                        // by the sketch bridge after point updates
                        // land. Reading parameters[0..4] here would
                        // index an empty vec.
                        continue;
                    }
                    EntityUpdate::Line(
                        Point2d::new(state.parameters[0], state.parameters[1]),
                        Vector2d::new(state.parameters[2], state.parameters[3]),
                    )
                }
                EntityRef::Circle(_) => {
                    if state.derived_center.is_some() {
                        derived_curves.push(entity);
                        continue;
                    }
                    EntityUpdate::Circle(
                        Point2d::new(state.parameters[0], state.parameters[1]),
                        state.parameters[2],
                    )
                }
                EntityRef::Arc(_) => {
                    if state.derived_arc_endpoints.is_some() || state.derived_center.is_some() {
                        derived_curves.push(entity);
                        continue;
                    }
                    EntityUpdate::Arc(
                        Point2d::new(state.parameters[0], state.parameters[1]),
                        state.parameters[2],
                        state.parameters[3],
                        state.parameters[4],
                    )
                }
                EntityRef::Rectangle(_) => EntityUpdate::Rectangle(
                    Point2d::new(state.parameters[0], state.parameters[1]),
                    state.parameters[2],
                    state.parameters[3],
                    state.parameters[4],
                ),
                EntityRef::Ellipse(_) => EntityUpdate::Ellipse(
                    Point2d::new(state.parameters[0], state.parameters[1]),
                    state.parameters[2],
                    state.parameters[3],
                    state.parameters[4],
                ),
                EntityRef::Spline(_) | EntityRef::Polyline(_) => {
                    EntityUpdate::Parameters(state.parameters.clone())
                }
            };

            updates.insert(entity, update);
        }

        // Shared-ref circles/arcs: geometry derives from the shared
        // points' SOLVED state (plus the entity's private scalars), so
        // by construction the written-back geometry cannot disagree
        // with the points. Missing point state or a degenerate chord
        // yields no update — the entity keeps its prior geometry
        // ("preserve identity, never poison the store").
        for entity in derived_curves {
            let update = match entity {
                EntityRef::Circle(_) => {
                    let (Some(center), Some(radius)) = (
                        self.get_circle_center(&entity),
                        self.get_circle_radius(&entity),
                    ) else {
                        continue;
                    };
                    EntityUpdate::Circle(center, radius)
                }
                EntityRef::Arc(_) => {
                    let (Some(center), Some(radius), Some((start_angle, end_angle))) = (
                        self.get_circle_center(&entity),
                        self.get_circle_radius(&entity),
                        self.get_arc_angles(&entity),
                    ) else {
                        continue;
                    };
                    EntityUpdate::Arc(center, radius, start_angle, end_angle)
                }
                _ => continue,
            };
            updates.insert(entity, update);
        }

        updates
    }

    /// Get constraint violations
    fn get_violations(&self) -> Vec<(ConstraintId, f64)> {
        let mut violations = Vec::new();

        for constraint in &self.constraints {
            let errors = self.evaluate_constraint_error(constraint);
            let error_magnitude = errors.iter().map(|e| e * e).sum::<f64>().sqrt();

            if error_magnitude > self.tolerance {
                violations.push((constraint.id, error_magnitude));
            }
        }

        violations
    }

    /// Evaluate tangent constraint between line and circle
    fn evaluate_tangent_constraint(&self, entity1: &EntityRef, entity2: &EntityRef) -> Vec<f64> {
        // Get line point and direction
        let line_entity = if self.is_line(entity1) {
            entity1
        } else {
            entity2
        };
        let circle_entity = if self.is_circle(entity1) {
            entity1
        } else {
            entity2
        };

        if let (Some(line_point), Some(line_dir), Some(circle_center), Some(radius)) = (
            self.get_line_point(line_entity),
            self.get_line_direction(line_entity),
            self.get_circle_center(circle_entity),
            self.get_circle_radius(circle_entity),
        ) {
            // Vector from circle center to line point
            let cp = Vector2d::from_points(&circle_center, &line_point);

            // Distance from center to line should equal radius for tangency
            // Using formula: |cp - (cp·d)d| = r where d is unit line direction
            let d_unit = line_dir.normalize().unwrap_or(Vector2d::UNIT_X);
            let proj = cp.dot(&d_unit);
            let offset = d_unit.scale(proj);
            let perp_vec = cp.sub(&offset);
            let perp_dist = perp_vec.magnitude();

            vec![perp_dist - radius]
        } else {
            vec![0.0]
        }
    }

    /// Evaluate equal constraint between entities
    fn evaluate_equal_constraint(&self, entity1: &EntityRef, entity2: &EntityRef) -> Vec<f64> {
        // Compare appropriate dimensions based on entity types
        match (entity1, entity2) {
            (EntityRef::Line(_), EntityRef::Line(_)) => {
                // Equal line lengths
                if let (Some(len1), Some(len2)) =
                    (self.get_line_length(entity1), self.get_line_length(entity2))
                {
                    vec![len1 - len2]
                } else {
                    vec![0.0]
                }
            }
            (EntityRef::Circle(_), EntityRef::Circle(_))
            | (EntityRef::Arc(_), EntityRef::Arc(_)) => {
                // Equal radii
                if let (Some(r1), Some(r2)) = (
                    self.get_circle_radius(entity1),
                    self.get_circle_radius(entity2),
                ) {
                    vec![r1 - r2]
                } else {
                    vec![0.0]
                }
            }
            (EntityRef::Rectangle(_), EntityRef::Rectangle(_)) => {
                // Equal dimensions: both width AND height must match.
                // Equating only the area or only one dimension would
                // leave one DOF un-pinned, which would surprise users
                // who expect "same rectangle shape". The 2-residual
                // shape is reported by `constraint_error_count` so the
                // Jacobian allocates the right row budget.
                if let (Some((w1, h1)), Some((w2, h2))) = (
                    self.get_rectangle_dimensions(entity1),
                    self.get_rectangle_dimensions(entity2),
                ) {
                    vec![w1 - w2, h1 - h2]
                } else {
                    vec![0.0, 0.0]
                }
            }
            (EntityRef::Ellipse(_), EntityRef::Ellipse(_)) => {
                // Equal axes: both semi_major AND semi_minor must
                // match. Rotation is independent, matching the
                // rectangle convention — two ellipses are "equal"
                // when they have the same shape, regardless of how
                // they are oriented on the sketch plane. The 2-residual
                // shape is reported by `constraint_error_count` so the
                // Jacobian allocates the right row budget.
                if let (Some((a1, b1)), Some((a2, b2))) = (
                    self.get_ellipse_axes(entity1),
                    self.get_ellipse_axes(entity2),
                ) {
                    vec![a1 - a2, b1 - b2]
                } else {
                    vec![0.0, 0.0]
                }
            }
            _ => vec![0.0],
        }
    }

    /// Evaluate symmetric constraint
    fn evaluate_symmetric_constraint(
        &self,
        entity1: &EntityRef,
        entity2: &EntityRef,
        axis: &EntityRef,
    ) -> Vec<f64> {
        // Get axis line parameters
        if let (Some(axis_point), Some(axis_dir)) =
            (self.get_line_point(axis), self.get_line_direction(axis))
        {
            let axis_normal = Vector2d::new(-axis_dir.y, axis_dir.x); // Perpendicular to axis

            // Get positions of entities to be made symmetric
            if let (Some(p1), Some(p2)) = (
                self.get_point_position(entity1),
                self.get_point_position(entity2),
            ) {
                // Reflect p1 across axis to get expected p2 position
                let to_p1 = Vector2d::from_points(&axis_point, &p1);
                let dist_to_axis = to_p1.dot(&axis_normal);
                let offset = axis_normal.scale(2.0 * dist_to_axis);
                let reflected = Point2d::new(p1.x - offset.x, p1.y - offset.y);

                vec![p2.x - reflected.x, p2.y - reflected.y]
            } else {
                vec![0.0, 0.0]
            }
        } else {
            vec![0.0, 0.0]
        }
    }

    /// Evaluate point on curve constraint
    fn evaluate_point_on_curve(
        &self,
        point_entity: &EntityRef,
        curve_entity: &EntityRef,
    ) -> Vec<f64> {
        let point = self.get_point_position(point_entity);

        match curve_entity {
            EntityRef::Line(_) => {
                if let (Some(p), Some(line_point), Some(line_dir)) = (
                    point,
                    self.get_line_point(curve_entity),
                    self.get_line_direction(curve_entity),
                ) {
                    // Point should lie on line: (p - line_point) × line_dir = 0
                    let to_point = Vector2d::from_points(&line_point, &p);
                    vec![to_point.cross(&line_dir)]
                } else {
                    vec![0.0]
                }
            }
            EntityRef::Circle(_) => {
                if let (Some(p), Some(center), Some(radius)) = (
                    point,
                    self.get_circle_center(curve_entity),
                    self.get_circle_radius(curve_entity),
                ) {
                    // Point should be at radius distance from center
                    let dist = Vector2d::from_points(&center, &p).magnitude();
                    vec![dist - radius]
                } else {
                    vec![0.0]
                }
            }
            EntityRef::Spline(_) => {
                // Project p onto the spline and return the *signed*
                // perpendicular distance: (p − foot) × tangent / |tangent|.
                // The sign comes from the cross-product direction so the
                // residual is monotonic through zero, which is what the
                // Newton-Raphson loop needs to converge (an unsigned
                // distance has a non-differentiable cusp at the
                // solution).
                //
                // Dispatch on weights metadata: NURBS uses native
                // rational closest_point + tangent; B-Spline uses
                // the non-rational path. Projecting NURBS weights
                // away (via `to_bspline`) would change the curve and
                // drive the foot to the wrong place — see
                // `spline2d::NurbsCurve2d::to_bspline` doc.
                let p = match point {
                    Some(p) => p,
                    None => return vec![0.0],
                };
                let tolerance = Tolerance2d::default();
                let (foot, tangent_res) = if self.is_spline_rational(curve_entity) {
                    let nurbs = match self.get_nurbs_curve2d(curve_entity) {
                        Some(c) => c,
                        None => return vec![0.0],
                    };
                    let (foot, u) = match nurbs.closest_point(&p, &tolerance) {
                        Ok(pair) => pair,
                        Err(_) => return vec![0.0],
                    };
                    (foot, nurbs.tangent(u))
                } else {
                    let bspline = match self.get_bspline2d(curve_entity) {
                        Some(c) => c,
                        None => return vec![0.0],
                    };
                    let (foot, u) = match bspline.closest_point(&p, &tolerance) {
                        Ok(pair) => pair,
                        Err(_) => return vec![0.0],
                    };
                    (foot, bspline.tangent(u))
                };
                let foot_to_p = Vector2d::from_points(&foot, &p);
                let tangent = match tangent_res {
                    Ok(t) => t,
                    Err(_) => return vec![foot_to_p.magnitude()],
                };
                let tan_len = tangent.magnitude();
                if tan_len < STRICT_TOLERANCE.distance() {
                    // Degenerate tangent (e.g. evaluated at a coincident
                    // knot) — fall back to unsigned distance. Direction
                    // is undefined here, so signedness adds no
                    // information.
                    return vec![foot_to_p.magnitude()];
                }
                vec![foot_to_p.cross(&tangent) / tan_len]
            }
            EntityRef::Polyline(_) => {
                // Project p onto the polyline and return the signed
                // perpendicular distance against the closest segment's
                // tangent: `(p − foot) × seg_dir / |seg_dir|`. Sign
                // comes from the cross-product direction so the
                // residual is monotonic through zero, matching the
                // spline path (an unsigned distance has a
                // non-differentiable cusp at the solution which
                // breaks Newton-Raphson convergence). At the
                // vertex-bend ridge (two segments equidistant) the
                // function is continuous but not smooth — same
                // limitation as a polyline geometrically has.
                let p = match point {
                    Some(p) => p,
                    None => return vec![0.0],
                };
                let polyline = match self.get_polyline2d(curve_entity) {
                    Some(pl) => pl,
                    None => return vec![0.0],
                };
                let (foot, seg_idx, _t) = polyline.closest_point(&p);
                let segment = match polyline.segment(seg_idx) {
                    Some(s) => s,
                    None => return vec![Vector2d::from_points(&foot, &p).magnitude()],
                };
                let seg_dir = Vector2d::from_points(&segment.start, &segment.end);
                let seg_len = seg_dir.magnitude();
                let foot_to_p = Vector2d::from_points(&foot, &p);
                if seg_len < STRICT_TOLERANCE.distance() {
                    // Degenerate segment (shouldn't happen — Polyline2d
                    // rejects coincident consecutive vertices on
                    // construction) — fall back to unsigned distance.
                    return vec![foot_to_p.magnitude()];
                }
                vec![foot_to_p.cross(&seg_dir) / seg_len]
            }
            _ => vec![0.0],
        }
    }

    /// Reconstruct a [`BSpline2d`] from the entity state when `entity`
    /// is a non-rational B-spline; `None` for NURBS (use
    /// [`ConstraintSolver::get_nurbs_curve2d`]) and every other kind.
    ///
    /// Reads control points as consecutive `(x, y)` pairs from
    /// `state.parameters` and pairs them with the structural
    /// `(degree, knots)` carried in `state.spline`. Any inconsistency
    /// (odd parameter length, wrong knot count, validation failure on
    /// `BSpline2d::new`) returns `None`, leaving the caller to fall
    /// back to a zero residual rather than panic.
    fn get_bspline2d(&self, entity: &EntityRef) -> Option<BSpline2d> {
        if !matches!(entity, EntityRef::Spline(_)) {
            return None;
        }
        let state = self.entity_state.get(entity)?;
        let meta = state.spline.as_ref()?;
        if meta.weights.is_some() {
            return None;
        }
        let control_points = control_points_from_parameters(&state.parameters)?;
        BSpline2d::new(meta.degree, control_points, meta.knots.clone()).ok()
    }

    /// Reconstruct a [`NurbsCurve2d`] from the entity state when
    /// `entity` is a rational NURBS curve; `None` for B-splines (use
    /// [`ConstraintSolver::get_bspline2d`]) and every other kind.
    ///
    /// Same control-point unpacking as [`Self::get_bspline2d`]; in
    /// addition the weights vector length must match the
    /// control-point count, otherwise `None` is returned (degrades
    /// the residual to zero rather than panicking).
    fn get_nurbs_curve2d(&self, entity: &EntityRef) -> Option<NurbsCurve2d> {
        if !matches!(entity, EntityRef::Spline(_)) {
            return None;
        }
        let state = self.entity_state.get(entity)?;
        let meta = state.spline.as_ref()?;
        let weights = meta.weights.as_ref()?;
        let control_points = control_points_from_parameters(&state.parameters)?;
        if weights.len() != control_points.len() {
            return None;
        }
        NurbsCurve2d::new(
            meta.degree,
            control_points,
            weights.clone(),
            meta.knots.clone(),
        )
        .ok()
    }

    /// Reconstruct a [`Polyline2d`] from the entity state when
    /// `entity` is a polyline; `None` for every other kind.
    ///
    /// Reads vertices as consecutive `(x, y)` pairs from
    /// `state.parameters` and pairs them with the `is_closed` flag
    /// carried in `state.polyline`. Any inconsistency (odd parameter
    /// length, validation failure on `Polyline2d::new`, e.g.
    /// coincident consecutive vertices that the solver drifted into)
    /// returns `None`, leaving the caller to fall back to a zero
    /// residual rather than panic.
    fn get_polyline2d(&self, entity: &EntityRef) -> Option<Polyline2d> {
        if !matches!(entity, EntityRef::Polyline(_)) {
            return None;
        }
        let state = self.entity_state.get(entity)?;
        let meta = state.polyline.as_ref()?;
        let vertices = control_points_from_parameters(&state.parameters)?;
        if vertices.len() < 2 {
            return None;
        }
        Polyline2d::new(vertices, meta.is_closed).ok()
    }

    /// `true` iff `entity` is a spline carrying NURBS weights.
    /// Drives the dispatch in
    /// [`Self::evaluate_point_on_curve`]; returns `false` for
    /// B-splines and every non-spline kind.
    fn is_spline_rational(&self, entity: &EntityRef) -> bool {
        if !matches!(entity, EntityRef::Spline(_)) {
            return false;
        }
        let Some(state) = self.entity_state.get(entity) else {
            return false;
        };
        state
            .spline
            .as_ref()
            .and_then(|m| m.weights.as_ref())
            .is_some()
    }

    /// Evaluate midpoint constraint
    fn evaluate_midpoint_constraint(
        &self,
        point_entity: &EntityRef,
        line_entity: &EntityRef,
    ) -> Vec<f64> {
        if let (Some(p), Some(line_start), Some(line_end)) = (
            self.get_point_position(point_entity),
            self.get_line_start(line_entity),
            self.get_line_end(line_entity),
        ) {
            // Point should be at midpoint of line
            let midpoint = line_start.midpoint(&line_end);
            vec![p.x - midpoint.x, p.y - midpoint.y]
        } else {
            vec![0.0, 0.0]
        }
    }

    /// Evaluate collinear constraint for three points
    fn evaluate_collinear_constraint(
        &self,
        p1: &EntityRef,
        p2: &EntityRef,
        p3: &EntityRef,
    ) -> Vec<f64> {
        if let (Some(pt1), Some(pt2), Some(pt3)) = (
            self.get_point_position(p1),
            self.get_point_position(p2),
            self.get_point_position(p3),
        ) {
            // Three points are collinear if cross product is zero
            let v1 = Vector2d::from_points(&pt1, &pt2);
            let v2 = Vector2d::from_points(&pt1, &pt3);
            vec![v1.cross(&v2)]
        } else {
            vec![0.0]
        }
    }

    /// Evaluate G1 continuity (tangent continuity)
    fn evaluate_g1_continuity(&self, curve1: &EntityRef, curve2: &EntityRef) -> Vec<f64> {
        // Get tangent vectors at connection point
        if let (Some(t1), Some(t2)) = (
            self.get_curve_tangent_at_end(curve1),
            self.get_curve_tangent_at_start(curve2),
        ) {
            // Tangents should be parallel (cross product = 0)
            vec![t1.cross(&t2), (t1.magnitude() - t2.magnitude()) * 0.1] // Also try to match magnitudes
        } else {
            vec![0.0, 0.0]
        }
    }

    /// Evaluate G2 continuity (curvature continuity).
    /// Returns the scalar curvature mismatch κ₁ − κ₂ at the join. G2 holds
    /// when this residual is zero. Higher-order terms (G3, G4...) are out
    /// of scope for the 2D constraint solver.
    fn evaluate_g2_continuity(&self, curve1: &EntityRef, curve2: &EntityRef) -> Vec<f64> {
        if let (Some(k1), Some(k2)) = (
            self.get_curve_curvature_at_end(curve1),
            self.get_curve_curvature_at_start(curve2),
        ) {
            vec![k1 - k2]
        } else {
            vec![0.0]
        }
    }

    // Helper methods for entity queries
    fn is_line(&self, entity: &EntityRef) -> bool {
        matches!(entity, EntityRef::Line(_))
    }

    fn is_circle(&self, entity: &EntityRef) -> bool {
        matches!(entity, EntityRef::Circle(_) | EntityRef::Arc(_))
    }

    fn get_line_point(&self, entity: &EntityRef) -> Option<Point2d> {
        if let EntityRef::Line(_id) = entity {
            if let Some((start, _end)) = self.derived_segment_of(entity) {
                return self.get_point_position(&start);
            }
            self.entity_state.get(entity).and_then(|state| {
                if state.parameters.len() >= 2 {
                    Some(Point2d::new(state.parameters[0], state.parameters[1]))
                } else {
                    None
                }
            })
        } else {
            None
        }
    }

    fn get_line_start(&self, entity: &EntityRef) -> Option<Point2d> {
        // Lines in this solver are parameterized as (point, direction); the
        // anchored point is the start.
        self.get_line_point(entity)
    }

    fn get_line_end(&self, entity: &EntityRef) -> Option<Point2d> {
        if let Some((_start, end)) = self.derived_segment_of(entity) {
            return self.get_point_position(&end);
        }
        // Legacy (point, direction) parameterisation has no length —
        // fabricate one. Real segments take the derived path above.
        if let (Some(start), Some(dir)) =
            (self.get_line_point(entity), self.get_line_direction(entity))
        {
            let scaled_dir = dir.scale(100.0);
            Some(Point2d::new(start.x + scaled_dir.x, start.y + scaled_dir.y))
        } else {
            None
        }
    }

    fn get_line_length(&self, entity: &EntityRef) -> Option<f64> {
        if let Some((start, end)) = self.derived_segment_of(entity) {
            let a = self.get_point_position(&start)?;
            let b = self.get_point_position(&end)?;
            return Some(a.distance_to(&b));
        }
        // Legacy (point, direction) lines carry no real length.
        Some(100.0)
    }

    /// Read the angular range stored on an arc's entity state.
    ///
    /// Arc parameter layout matches the constructor in `EntityState::arc`:
    /// `[center.x, center.y, radius, start_angle, end_angle]`. Returns
    /// `None` if the entity is not an arc or its state is malformed.
    fn get_arc_angles(&self, entity: &EntityRef) -> Option<(f64, f64)> {
        match entity {
            EntityRef::Arc(_) => {
                // Shared-variable dispatch (SKETCH-DCM #45 Slice 1):
                // endpoint-derived arcs compute angles from the shared
                // points; shared-center arcs store them at
                // `parameters[1..3]` (`[r, a0, a1]` layout).
                if self.derived_arc_endpoints_of(entity).is_some() {
                    return self
                        .derived_arc_geometry(entity)
                        .map(|(_, _, a0, a1)| (a0, a1));
                }
                if self.derived_center_of(entity).is_some() {
                    return self.entity_state.get(entity).and_then(|state| {
                        if state.parameters.len() >= 3 {
                            Some((state.parameters[1], state.parameters[2]))
                        } else {
                            None
                        }
                    });
                }
                self.entity_state.get(entity).and_then(|state| {
                    if state.parameters.len() >= 5 {
                        Some((state.parameters[3], state.parameters[4]))
                    } else {
                        None
                    }
                })
            }
            _ => None,
        }
    }

    /// Tangent at the curve's end parameter (CCW orientation).
    ///
    /// For a line the tangent is the stored direction. For an arc the
    /// tangent at angle θ is `(-sin θ, cos θ)`. For a full circle the
    /// "end" coincides with the "start" at θ = 0 (CCW), giving `(0, 1)`.
    fn get_curve_tangent_at_end(&self, entity: &EntityRef) -> Option<Vector2d> {
        match entity {
            EntityRef::Line(_) => self.get_line_direction(entity),
            EntityRef::Arc(_) => {
                let (_, end_angle) = self.get_arc_angles(entity)?;
                Some(Vector2d::new(-end_angle.sin(), end_angle.cos()))
            }
            EntityRef::Circle(_) => {
                // Closed curve: end parameter at θ = 2π wraps to θ = 0.
                Some(Vector2d::new(0.0, 1.0))
            }
            _ => None,
        }
    }

    /// Tangent at the curve's start parameter (CCW orientation).
    fn get_curve_tangent_at_start(&self, entity: &EntityRef) -> Option<Vector2d> {
        match entity {
            EntityRef::Line(_) => self.get_line_direction(entity),
            EntityRef::Arc(_) => {
                let (start_angle, _) = self.get_arc_angles(entity)?;
                Some(Vector2d::new(-start_angle.sin(), start_angle.cos()))
            }
            EntityRef::Circle(_) => Some(Vector2d::new(0.0, 1.0)),
            _ => None,
        }
    }

    /// Signed curvature at the curve's end parameter.
    ///
    /// Lines have zero curvature. Circles and arcs traversed CCW have
    /// curvature `+1/r`; the constraint solver does not currently track
    /// arc orientation, so the unsigned `1/r` value is returned. Other
    /// curve types fall through with `None` so callers (G2 evaluators)
    /// can treat them as unsupported rather than silently mis-classifying.
    fn get_curve_curvature_at_end(&self, entity: &EntityRef) -> Option<f64> {
        match entity {
            EntityRef::Circle(_) | EntityRef::Arc(_) => {
                self.get_circle_radius(entity).map(|r| 1.0 / r)
            }
            EntityRef::Line(_) => Some(0.0),
            _ => None,
        }
    }

    /// Signed curvature at the curve's start parameter.
    ///
    /// Circles and arcs have constant curvature, so this matches
    /// `get_curve_curvature_at_end` exactly. For non-uniform curves
    /// (splines, ellipses) this would diverge from the end value; those
    /// kinds currently return `None` from both methods.
    fn get_curve_curvature_at_start(&self, entity: &EntityRef) -> Option<f64> {
        self.get_curve_curvature_at_end(entity)
    }
}

/// Map a `ConstraintPriority` to a numerical weight applied to its
/// row(s) in the Jacobian and corresponding residual entries.
///
/// The ratios are chosen so the weighted least-squares objective
/// respects the documented semantics:
/// - `Required` and `High` carry full weight — they are the
///   sketch's actual rigid constraints and must be solved exactly
///   when feasible.
/// - `Medium` and `Low` are best-effort. They participate in the
///   Newton step but are dominated when they conflict with a
///   higher-priority constraint. This is how a "soft" drag target
///   stays subordinate to the persistent constraints already on the
///   sketch — pulling the dragged point toward the cursor only as
///   far as the rigid constraint manifold allows.
fn priority_weight(p: ConstraintPriority) -> f64 {
    match p {
        ConstraintPriority::Required => 1.0,
        ConstraintPriority::High => 1.0,
        ConstraintPriority::Medium => 1e-2,
        ConstraintPriority::Low => 1e-3,
    }
}

impl EntityState {
    /// Number of free (non-fixed) parameters.
    fn free_param_count(&self) -> usize {
        self.fixed_mask.iter().filter(|&&fixed| !fixed).count()
    }

    /// Clone with every parameter frozen — placed geometry inside a
    /// DR-plan step solver (SKETCH-DCM #45 Slice 3): the step can read
    /// it through the residual accessors but Newton sees zero columns
    /// from it.
    fn frozen(&self) -> Self {
        let mut clone = self.clone();
        clone.fixed_mask = vec![true; clone.fixed_mask.len()];
        clone
    }

    /// Entities this state structurally shares variables with
    /// (the Slice-1 shared-variable model): a derived segment's or
    /// endpoint-derived arc's endpoints, a shared center's point.
    fn derived_refs(&self) -> Vec<EntityRef> {
        let mut refs = Vec::new();
        if let Some((start, end)) = self.derived_segment {
            refs.push(start);
            refs.push(end);
        }
        if let Some((start, end)) = self.derived_arc_endpoints {
            refs.push(start);
            refs.push(end);
        }
        if let Some(center) = self.derived_center {
            refs.push(center);
        }
        refs
    }

    /// Create state for a point
    pub fn point(pos: Point2d, fixed: bool) -> Self {
        Self {
            parameters: vec![pos.x, pos.y],
            fixed_mask: vec![fixed, fixed],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for a line
    pub fn line(point: Point2d, direction: Vector2d, point_fixed: bool, dir_fixed: bool) -> Self {
        Self {
            parameters: vec![point.x, point.y, direction.x, direction.y],
            fixed_mask: vec![point_fixed, point_fixed, dir_fixed, dir_fixed],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for a segment line DERIVED from two point
    /// entities (shared-variable model). Contributes ZERO degrees of
    /// freedom: its geometry is a pure function of the endpoint
    /// points' parameters.
    pub fn segment_between(start: EntityRef, end: EntityRef) -> Self {
        Self {
            parameters: Vec::new(),
            fixed_mask: Vec::new(),
            spline: None,
            polyline: None,
            derived_segment: Some((start, end)),
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for an arc DERIVED from two shared endpoint point
    /// entities (shared-variable model, SKETCH-DCM #45 Slice 1).
    ///
    /// # Parameterization: chord-frame center offset
    ///
    /// The single private parameter is `t = (C − M) · p̂`, the SIGNED
    /// offset of the arc's center `C` from the chord midpoint
    /// `M = (S + E) / 2` along the chord's left-hand unit
    /// perpendicular `p̂ = rot90((E − S) / ‖E − S‖)`. Everything else
    /// derives:
    ///
    /// ```text
    ///     C  = M + t·p̂
    ///     r  = √(t² + (‖E − S‖ / 2)²)
    ///     θ₀ = atan2(S − C),  θ₁ = atan2(E − C)
    /// ```
    ///
    /// Why this variant and not the alternatives named in the Slice-1
    /// spec:
    ///
    /// - **(center point, r) with derived endpoint angles** would keep
    ///   the arc's center/radius as private (or shared-point) state
    ///   and require the solver to hold `‖S − C‖ = r = ‖E − C‖` as two
    ///   IMPLICIT residuals. That is residual-mediated internal
    ///   consistency — the exact failure mode this slice exists to
    ///   remove — and it forces implicit-constraint bookkeeping into
    ///   every DOF count. Refused.
    /// - **(r, bulge)** (bulge = tan(sweep/4)) is also 1-DOF but blows
    ///   up as the sweep approaches a full turn and degenerates
    ///   non-smoothly near zero sweep. The chord offset `t` is smooth
    ///   and total: every center position on the perpendicular
    ///   bisector — every radius ≥ half-chord, minor and major arcs,
    ///   both bulge sides — is exactly one finite `t`. Its only
    ///   singularity is `S = E` (no chord), which is a degenerate arc
    ///   the creation API already rejects; if solving drives the
    ///   points transiently coincident the accessors return `None`
    ///   and the affected residuals degrade to zero for that
    ///   iteration instead of emitting NaN.
    ///
    /// Structural consequence (the point of the slice): the arc
    /// contributes exactly 1 DOF; its endpoint coordinates are counted
    /// once, in the shared points; DOF arithmetic is pure counting
    /// with zero implicit constraints — decomposition-ready.
    ///
    /// The `ccw` orientation bit is NOT solver state (discrete, not
    /// differentiable) — the sketch bridge preserves it across solve
    /// cycles exactly as for legacy arcs.
    pub fn arc_between(start: EntityRef, end: EntityRef, center_offset: f64) -> Self {
        Self {
            parameters: vec![center_offset],
            fixed_mask: vec![false],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: Some((start, end)),
            derived_center: None,
        }
    }

    /// Create state for an arc whose CENTER is a shared point entity
    /// (shared-variable model, SKETCH-DCM #45 Slice 1).
    ///
    /// Parameter layout is `[radius, start_angle, end_angle]` — the
    /// center's coordinates live in the referenced point entity and
    /// are read through [`ConstraintSolver::get_circle_center`]'s
    /// derived dispatch. 3 private DOF + 2 in the point = the arc's
    /// full 5, counted once.
    pub fn arc_centered(center: EntityRef, radius: f64, start_angle: f64, end_angle: f64) -> Self {
        Self {
            parameters: vec![radius, start_angle, end_angle],
            fixed_mask: vec![false, false, false],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: Some(center),
        }
    }

    /// Create state for a circle whose CENTER is a shared point entity
    /// (shared-variable model, SKETCH-DCM #45 Slice 1).
    ///
    /// Parameter layout is `[radius]` — the center's coordinates live
    /// in the referenced point entity, once, so circles referencing
    /// the same point are concentric by construction.
    pub fn circle_centered(center: EntityRef, radius: f64) -> Self {
        Self {
            parameters: vec![radius],
            fixed_mask: vec![false],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: Some(center),
        }
    }

    /// Create state for a circle
    pub fn circle(center: Point2d, radius: f64, center_fixed: bool, radius_fixed: bool) -> Self {
        Self {
            parameters: vec![center.x, center.y, radius],
            fixed_mask: vec![center_fixed, center_fixed, radius_fixed],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for an arc.
    ///
    /// Parameter layout is `[center.x, center.y, radius, start_angle,
    /// end_angle]` — matches the comment on `get_arc_angles` and the
    /// `EntityUpdate::Arc(center, radius, start_angle, end_angle)`
    /// emission path. `ccw` is not stored as a solver parameter
    /// (orientation is a discrete bit, not a continuously-varying
    /// quantity the solver could differentiate over); the sketch
    /// bridge preserves it across solve cycles.
    ///
    /// The three fix flags follow the same shape as `circle(…)`:
    /// `center_fixed` pins both x and y; `radius_fixed` pins the
    /// scalar; `angles_fixed` pins both angular endpoints together.
    /// Splitting start/end fix into separate flags would be a no-op
    /// for every constraint that currently exists (tangent / equal /
    /// radius / concentric / point-on-curve all leave the angles
    /// free), so the simpler API ships first; refine if a user
    /// surface needs per-endpoint pinning later.
    pub fn arc(
        center: Point2d,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
        center_fixed: bool,
        radius_fixed: bool,
        angles_fixed: bool,
    ) -> Self {
        Self {
            parameters: vec![center.x, center.y, radius, start_angle, end_angle],
            fixed_mask: vec![
                center_fixed,
                center_fixed,
                radius_fixed,
                angles_fixed,
                angles_fixed,
            ],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for a rectangle.
    ///
    /// Parameter layout is `[center.x, center.y, width, height,
    /// rotation]` — matches `EntityUpdate::Rectangle(center, width,
    /// height, rotation)` and the emission path in
    /// `get_entity_updates`. Because the layout puts the center at
    /// `params[0..=1]`, `get_point_position` already returns the
    /// rectangle's center, which means `Coincident`, `Distance`,
    /// `XCoordinate`, and `YCoordinate` constraints work for
    /// rectangles without any further dispatch.
    ///
    /// Four fix flags:
    /// - `center_fixed` pins both x and y of the center together.
    /// - `width_fixed` / `height_fixed` pin the scalar dimensions
    ///   independently — a rectangle with width fixed can still flex
    ///   in height, which is what the snap engine wants when an
    ///   Equal constraint propagates only one dimension.
    /// - `rotation_fixed` pins the rotation angle.
    pub fn rectangle(
        center: Point2d,
        width: f64,
        height: f64,
        rotation: f64,
        center_fixed: bool,
        width_fixed: bool,
        height_fixed: bool,
        rotation_fixed: bool,
    ) -> Self {
        Self {
            parameters: vec![center.x, center.y, width, height, rotation],
            fixed_mask: vec![
                center_fixed,
                center_fixed,
                width_fixed,
                height_fixed,
                rotation_fixed,
            ],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for an ellipse.
    ///
    /// Parameter layout is `[center.x, center.y, semi_major, semi_minor,
    /// rotation]` — matches `EntityUpdate::Ellipse(center, semi_major,
    /// semi_minor, rotation)` and the emission path in
    /// `get_entity_updates`. The leading two slots are the centre, so
    /// `get_point_position` already returns the ellipse centre and
    /// `Coincident`, `Distance`, `XCoordinate`, and `YCoordinate`
    /// constraints work for ellipses without any further dispatch.
    /// `get_circle_center` also matches `EntityRef::Ellipse`, which
    /// makes `Concentric` work between any pair of {Circle, Arc,
    /// Rectangle, Ellipse}.
    ///
    /// Four fix flags:
    /// - `center_fixed` pins both x and y of the centre together.
    /// - `semi_major_fixed` / `semi_minor_fixed` pin the scalar axis
    ///   lengths independently — an ellipse with `semi_major` fixed
    ///   can still flex in `semi_minor`, which is what the snap
    ///   engine wants when an Equal constraint propagates only one
    ///   dimension.
    /// - `rotation_fixed` pins the rotation angle.
    ///
    /// The `semi_major >= semi_minor` convention enforced by
    /// `Ellipse2d::new` is applied only on write-back (see
    /// `apply_solver_result`); inside the solver `semi_major` and
    /// `semi_minor` are independent free parameters, which keeps the
    /// Jacobian well-conditioned.
    pub fn ellipse(
        center: Point2d,
        semi_major: f64,
        semi_minor: f64,
        rotation: f64,
        center_fixed: bool,
        semi_major_fixed: bool,
        semi_minor_fixed: bool,
        rotation_fixed: bool,
    ) -> Self {
        Self {
            parameters: vec![center.x, center.y, semi_major, semi_minor, rotation],
            fixed_mask: vec![
                center_fixed,
                center_fixed,
                semi_major_fixed,
                semi_minor_fixed,
                rotation_fixed,
            ],
            spline: None,
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for a non-rational B-Spline curve.
    ///
    /// Parameter layout is `[cp0.x, cp0.y, cp1.x, cp1.y, …, cpn.x,
    /// cpn.y]` — 2n entries for n control points. The 2D solver treats
    /// each `(x, y)` pair as two independent free parameters, so an
    /// unconstrained B-spline with n control points contributes 2n
    /// DOFs — exactly matching [`ParametricSpline2d::degrees_of_freedom`].
    ///
    /// Knots and degree are pinned across solves (see
    /// [`SplineMetadata`]). The kernel needs them to reconstruct a
    /// [`BSpline2d`] when evaluating `PointOnCurve` residuals; varying
    /// them would be a structural edit (refine / elevate) outside the
    /// constraint-solver contract.
    ///
    /// `fixed_control_points` pins every control point's x and y
    /// together. Per-control-point pinning is a no-op for every
    /// currently-defined constraint (PointOnCurve / Coincident-to-first
    /// don't need it) so the simpler API ships first; refine if a user
    /// surface needs per-CP locking later.
    ///
    /// **NURBS:** rational NURBS curves are deferred to a follow-up
    /// slice. The naive route (project through [`NurbsCurve2d::to_bspline`])
    /// loses the rational-denominator term and changes the curve's
    /// geometry, so it would drive the solver toward the wrong point.
    /// A native rational `closest_point` on `NurbsCurve2d` lands first,
    /// then a sibling `spline_nurbs` constructor wires it into the
    /// solver.
    pub fn spline_bspline(
        degree: usize,
        control_points: Vec<Point2d>,
        knots: Vec<f64>,
        fixed_control_points: bool,
    ) -> Self {
        let mut parameters = Vec::with_capacity(control_points.len() * 2);
        let mut fixed_mask = Vec::with_capacity(control_points.len() * 2);
        for cp in &control_points {
            parameters.push(cp.x);
            parameters.push(cp.y);
            fixed_mask.push(fixed_control_points);
            fixed_mask.push(fixed_control_points);
        }
        Self {
            parameters,
            fixed_mask,
            spline: Some(SplineMetadata {
                degree,
                knots,
                weights: None,
            }),
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for a rational NURBS curve.
    ///
    /// Parameter layout matches [`EntityState::spline_bspline`]: a
    /// flat `[cp0.x, cp0.y, cp1.x, cp1.y, …]` pack of 2n entries for
    /// n control points. Weights live in [`SplineMetadata::weights`]
    /// and are pinned (see that field's doc). The solver therefore
    /// reports 2n DOFs for an unconstrained NURBS curve — the same
    /// as a B-Spline of equal control-point count. Per-weight free
    /// solving is a follow-up slice (would need a 3n pack +
    /// non-negativity guard on each Newton step).
    ///
    /// `weights.len()` MUST equal `control_points.len()`. Mismatch is
    /// rejected at curve-reconstruction time in
    /// [`ConstraintSolver::get_nurbs_curve2d`] (returns `None`,
    /// degrading the residual to zero rather than panicking).
    pub fn spline_nurbs(
        degree: usize,
        control_points: Vec<Point2d>,
        weights: Vec<f64>,
        knots: Vec<f64>,
        fixed_control_points: bool,
    ) -> Self {
        let mut parameters = Vec::with_capacity(control_points.len() * 2);
        let mut fixed_mask = Vec::with_capacity(control_points.len() * 2);
        for cp in &control_points {
            parameters.push(cp.x);
            parameters.push(cp.y);
            fixed_mask.push(fixed_control_points);
            fixed_mask.push(fixed_control_points);
        }
        Self {
            parameters,
            fixed_mask,
            spline: Some(SplineMetadata {
                degree,
                knots,
                weights: Some(weights),
            }),
            polyline: None,
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }

    /// Create state for a polyline (piecewise-linear curve).
    ///
    /// Parameter layout is `[v0.x, v0.y, v1.x, v1.y, …, vn.x, vn.y]` —
    /// 2n entries for n vertices. The 2D solver treats each `(x, y)`
    /// pair as two independent free parameters, so an unconstrained
    /// polyline with n vertices contributes 2n DOFs — exactly matching
    /// [`ParametricPolyline2d::degrees_of_freedom`].
    ///
    /// The `is_closed` flag is pinned across solves (see
    /// [`PolylineMetadata`]). The kernel needs it to reconstruct a
    /// [`Polyline2d`] when evaluating `PointOnCurve` residuals;
    /// changing it (open ↔ closed) is a structural edit (adding or
    /// removing the wrap-around segment) outside the constraint-solver
    /// contract.
    ///
    /// `fixed_vertices` pins every vertex's x and y together.
    /// Per-vertex pinning is a no-op for every currently-defined
    /// constraint (PointOnCurve does not need it) so the simpler API
    /// ships first; refine if a user surface needs per-vertex locking
    /// later.
    pub fn polyline(vertices: Vec<Point2d>, is_closed: bool, fixed_vertices: bool) -> Self {
        let mut parameters = Vec::with_capacity(vertices.len() * 2);
        let mut fixed_mask = Vec::with_capacity(vertices.len() * 2);
        for v in &vertices {
            parameters.push(v.x);
            parameters.push(v.y);
            fixed_mask.push(fixed_vertices);
            fixed_mask.push(fixed_vertices);
        }
        Self {
            parameters,
            fixed_mask,
            spline: None,
            polyline: Some(PolylineMetadata { is_closed }),
            derived_segment: None,
            derived_arc_endpoints: None,
            derived_center: None,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Coverage tests for the 2D constraint solver.
    //!
    //! These tests exercise:
    //! - Solver lifecycle / configuration (Category A)
    //! - Convergence-status enumeration (Category B)
    //! - Geometric-constraint evaluators (Category C)
    //! - Dimensional-constraint evaluators (Category D)
    //! - Jacobian computation via numerical differentiation (Category E)
    //! - Gaussian elimination (Category F)
    //! - Parameter-update damping and the fixed mask (Category G)
    //! - Violation reporting (Category H)
    //! - Robustness / degenerate inputs (Category I)
    //!
    //! Tests use only the public surface of [`ConstraintSolver`] and
    //! [`EntityState`]; the Polyline entity kind whose state cannot
    //! yet be authored through the public API is exercised indirectly
    //! via constraints it shares with the supported kinds
    //! (Point, Line, Circle, Arc, Rectangle, Ellipse, Spline).
    //! B-Spline support landed in slice C-4-a-1; rational NURBS support
    //! (native rational `closest_point` + `tangent` on `NurbsCurve2d`)
    //! landed in slice C-4-a-2.
    #![allow(clippy::float_cmp)]
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::sketch2d::constraints::{ConstraintPriority, ConstraintType};
    use crate::sketch2d::spline2d::Spline2dId;
    use crate::sketch2d::{Circle2dId, Ellipse2dId, Line2dId, Point2dId, Rectangle2dId};

    // ────────────────────────────── helpers ───────────────────────────

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    fn point_ref() -> EntityRef {
        EntityRef::Point(Point2dId::new())
    }

    fn line_ref() -> EntityRef {
        EntityRef::Line(Line2dId::new())
    }

    fn circle_ref() -> EntityRef {
        EntityRef::Circle(Circle2dId::new())
    }

    fn rect_ref() -> EntityRef {
        EntityRef::Rectangle(Rectangle2dId::new())
    }

    fn ellipse_ref() -> EntityRef {
        EntityRef::Ellipse(Ellipse2dId::new())
    }

    fn spline_ref() -> EntityRef {
        EntityRef::Spline(Spline2dId::new())
    }

    /// Build the clamped-uniform B-spline state used by the
    /// `point_on_bspline_*` tests: degree-2, four control points at
    /// (0,0), (1,2), (2,0), (3,2), open-uniform knot vector
    /// `[0, 0, 0, 0.5, 1, 1, 1]`. The curve interpolates the first
    /// and last control points (clamped endpoints) and bulges between
    /// them, which gives a non-degenerate tangent at every interior
    /// parameter.
    fn sample_bspline_state() -> EntityState {
        let control_points = vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 2.0),
            Point2d::new(2.0, 0.0),
            Point2d::new(3.0, 2.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0];
        EntityState::spline_bspline(2, control_points, knots, false)
    }

    fn coincident(p1: EntityRef, p2: EntityRef) -> Constraint {
        Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![p1, p2],
            ConstraintPriority::High,
        )
    }

    fn distance(p1: EntityRef, p2: EntityRef, d: f64) -> Constraint {
        Constraint::new_dimensional(
            DimensionalConstraint::Distance(d),
            vec![p1, p2],
            ConstraintPriority::High,
        )
    }

    // ─────────── RED: silent-DOF-lie / inert-residual regression ───────
    //
    // Before the inert-constraint fix these constraints were DOF-counted
    // but their residual body was `_ => vec![0.0]` — a violated constraint
    // reported ZERO residual, so the solver called the sketch solved. Each
    // test below evaluates the constraint on a configuration that VIOLATES
    // it and asserts the residual is non-zero. They fail (residual 0) on the
    // inert implementation and pass once the real equation is wired in.

    fn residual_mag(errs: &[f64]) -> f64 {
        errs.iter().map(|e| e * e).sum::<f64>().sqrt()
    }

    #[test]
    fn red_diameter_residual_nonzero_when_violated() {
        let s = ConstraintSolver::new();
        let c = circle_ref();
        // radius 3 → diameter 6, but we demand diameter 10 (violated by 4).
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 3.0, false, false),
        );
        let errs = s.evaluate_dimensional_constraint(&DimensionalConstraint::Diameter(10.0), &[c]);
        assert!(
            residual_mag(&errs) > 1e-9,
            "Diameter must produce a non-zero residual when violated, got {errs:?}"
        );
    }

    #[test]
    fn red_length_residual_nonzero_when_violated() {
        let s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let line = line_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(3.0, 4.0), false)); // length 5
        s.add_entity(line, EntityState::segment_between(a, b));
        let errs = s.evaluate_dimensional_constraint(&DimensionalConstraint::Length(10.0), &[line]);
        assert!(
            residual_mag(&errs) > 1e-9,
            "Length must produce a non-zero residual when violated, got {errs:?}"
        );
    }

    #[test]
    fn red_area_residual_nonzero_when_violated() {
        let s = ConstraintSolver::new();
        let c = circle_ref();
        // area = π·2² ≈ 12.566, demand 100 (violated).
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 2.0, false, false),
        );
        let errs = s.evaluate_dimensional_constraint(&DimensionalConstraint::Area(100.0), &[c]);
        assert!(
            residual_mag(&errs) > 1e-9,
            "Area must produce a non-zero residual when violated, got {errs:?}"
        );
    }

    // ── GREEN: real residual drives the solver to satisfy the constraint ──

    fn residual_of(s: &ConstraintSolver, c: &Constraint) -> f64 {
        residual_mag(&s.evaluate_constraint_error(c))
    }

    #[test]
    fn diameter_solver_drives_radius_to_target() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        // Centre fixed, radius free. Diameter(10) ⇒ radius 5.
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 3.0, true, false),
        );
        let con = Constraint::new_dimensional(
            DimensionalConstraint::Diameter(10.0),
            vec![c],
            ConstraintPriority::High,
        );
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        let r = s.get_circle_radius(&c).expect("radius");
        assert!(approx_eq(r, 5.0, 1e-6), "radius should solve to 5, got {r}");
        assert!(residual_of(&s, &con) < 1e-6);
    }

    #[test]
    fn length_solver_drives_segment_to_target() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let line = line_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), true)); // anchor fixed
        s.add_entity(b, EntityState::point(Point2d::new(3.0, 4.0), false)); // free (len 5)
        s.add_entity(line, EntityState::segment_between(a, b));
        let con = Constraint::new_dimensional(
            DimensionalConstraint::Length(10.0),
            vec![line],
            ConstraintPriority::High,
        );
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        let len = s.get_line_length(&line).expect("length");
        assert!(
            approx_eq(len, 10.0, 1e-6),
            "length should solve to 10, got {len}"
        );
    }

    #[test]
    fn area_solver_drives_radius() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 2.0, true, false),
        );
        let target = 100.0;
        let con = Constraint::new_dimensional(
            DimensionalConstraint::Area(target),
            vec![c],
            ConstraintPriority::High,
        );
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        let r = s.get_circle_radius(&c).expect("radius");
        let expected = (target / std::f64::consts::PI).sqrt();
        assert!(
            approx_eq(r, expected, 1e-5),
            "radius should solve to {expected}, got {r}"
        );
    }

    #[test]
    fn curvature_solver_drives_radius() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 5.0, true, false),
        );
        // κ = 0.5 ⇒ r = 2.
        let con = Constraint::new_dimensional(
            DimensionalConstraint::Curvature(0.5),
            vec![c],
            ConstraintPriority::High,
        );
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        let r = s.get_circle_radius(&c).expect("radius");
        assert!(approx_eq(r, 2.0, 1e-5), "radius should solve to 2, got {r}");
    }

    #[test]
    fn center_of_mass_solver_moves_circle_centre() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        // Centre free at (1,1); pin centroid to (0,0).
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(1.0, 1.0), 2.0, false, true),
        );
        let con = Constraint::new_dimensional(
            DimensionalConstraint::CenterOfMass { x: 0.0, y: 0.0 },
            vec![c],
            ConstraintPriority::High,
        );
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        let centre = s.get_circle_center(&c).expect("centre");
        assert!(
            approx_eq(centre.x, 0.0, 1e-6) && approx_eq(centre.y, 0.0, 1e-6),
            "centre should solve to origin, got {centre:?}"
        );
    }

    #[test]
    fn aspect_ratio_residual_nonzero_then_solves_for_rectangle() {
        let mut s = ConstraintSolver::new();
        let r = rect_ref();
        // width 1, height 1 (ratio 1); demand ratio 2. Width free.
        s.add_entity(
            r,
            EntityState::rectangle(
                Point2d::new(0.0, 0.0),
                1.0,
                1.0,
                0.0,
                true,
                false,
                true,
                true,
            ),
        );
        let con = Constraint::new_dimensional(
            DimensionalConstraint::AspectRatio(2.0),
            vec![r],
            ConstraintPriority::High,
        );
        assert!(
            residual_of(&s, &con) > 1e-9,
            "ratio 1 vs demanded 2 must be violated"
        );
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        assert!(residual_of(&s, &con) < 1e-6, "aspect-ratio should solve");
    }

    #[test]
    fn equal_area_residual_nonzero_when_radii_differ() {
        let s = ConstraintSolver::new();
        let c1 = circle_ref();
        let c2 = circle_ref();
        s.add_entity(
            c1,
            EntityState::circle(Point2d::new(0.0, 0.0), 2.0, false, false),
        );
        s.add_entity(
            c2,
            EntityState::circle(Point2d::new(9.0, 0.0), 3.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::EqualArea, &[c1, c2]);
        assert!(
            residual_mag(&errs) > 1e-9,
            "unequal areas must be nonzero, got {errs:?}"
        );
    }

    #[test]
    fn centroid_constraint_pins_point_to_circle_centre() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let c = circle_ref();
        s.add_entity(p, EntityState::point(Point2d::new(4.0, 4.0), false));
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 2.0, true, true),
        );
        let con = Constraint::new_geometric(
            GeometricConstraint::Centroid,
            vec![p, c],
            ConstraintPriority::High,
        );
        // 2 residual rows, matching `constraint_error_count`.
        assert_eq!(s.evaluate_constraint_error(&con).len(), 2);
        assert!(residual_of(&s, &con) > 1e-9, "point off centre is violated");
        s.set_constraints(vec![con.clone()]);
        let _ = s.solve();
        let pos = s.get_point_position(&p).expect("point");
        assert!(
            approx_eq(pos.x, 0.0, 1e-6) && approx_eq(pos.y, 0.0, 1e-6),
            "point should land on centre, got {pos:?}"
        );
    }

    #[test]
    fn unsupported_constraint_refuses_solve_and_reports_violation() {
        // MinDistance is recognised but has no residual equation. It must
        // NOT be silently reported as satisfied: the solver emits an
        // irreducible residual and never converges, and it removes 0 DOF.
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), true));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 0.0), true));
        let con = Constraint::new_dimensional(
            DimensionalConstraint::MinDistance(5.0),
            vec![a, b],
            ConstraintPriority::High,
        );
        // Irreducible non-zero residual (the refusal marker).
        let errs = s.evaluate_constraint_error(&con);
        assert!(
            residual_mag(&errs) > 1e-3,
            "unsupported constraint must not read as satisfied"
        );
        // Removes zero DOF — cannot fake a fully-constrained verdict.
        assert_eq!(con.degrees_of_freedom_removed(), 0);
        assert!(!con.constraint_type.is_numerically_enforced());
        s.set_constraints(vec![con.clone()]);
        let r = s.solve();
        assert!(
            !matches!(r.status, SolverStatus::Converged { .. }),
            "solver must never claim Converged for an unsupported constraint: {:?}",
            r.status
        );
        assert!(
            !r.violations.is_empty(),
            "unsupported constraint must surface as a violation"
        );
    }

    // ───────────────────── A. Lifecycle & configuration ───────────────

    #[test]
    fn solver_new_has_zero_entities_and_constraints() {
        let s = ConstraintSolver::new();
        assert_eq!(s.entity_state.len(), 0);
        assert_eq!(s.constraints.len(), 0);
        assert_eq!(s.dependency_graph.len(), 0);
    }

    #[test]
    fn solver_default_max_iterations_is_100() {
        let s = ConstraintSolver::new();
        assert_eq!(s.max_iterations, 100);
    }

    #[test]
    fn solver_default_tolerance_is_1e_minus_10() {
        let s = ConstraintSolver::new();
        assert_eq!(s.tolerance, 1e-10);
    }

    #[test]
    fn set_max_iterations_updates_field() {
        let mut s = ConstraintSolver::new();
        s.set_max_iterations(42);
        assert_eq!(s.max_iterations, 42);
    }

    #[test]
    fn set_tolerance_updates_field() {
        let mut s = ConstraintSolver::new();
        s.set_tolerance(1e-3);
        assert_eq!(s.tolerance, 1e-3);
    }

    #[test]
    fn add_entity_inserts_into_dashmap() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(1.0, 2.0), false));
        assert!(s.entity_state.contains_key(&p));
    }

    #[test]
    fn set_constraints_builds_dependency_graph() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let c = coincident(a, b);
        let cid = c.id;
        s.set_constraints(vec![c]);
        let deps = s.dependency_graph.get(&cid).expect("graph entry");
        assert!(deps.contains(&a));
        assert!(deps.contains(&b));
    }

    // ────────────────── B. Convergence-status enumeration ─────────────

    #[test]
    fn empty_system_converges_immediately() {
        let mut s = ConstraintSolver::new();
        let r = s.solve();
        match r.status {
            SolverStatus::Converged { iterations, .. } => assert_eq!(iterations, 0),
            other => panic!("expected Converged, got {:?}", other),
        }
    }

    #[test]
    fn coincident_two_free_points_converges() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(2.0, 0.0), false));
        s.set_constraints(vec![coincident(a, b)]);
        let r = s.solve();
        // The system is under-constrained (4 DOF, 2 equations), so
        // check_constraint_count reports it before iteration starts.
        assert!(matches!(
            r.status,
            SolverStatus::UnderConstrained { .. } | SolverStatus::Converged { .. }
        ));
    }

    #[test]
    fn coincident_one_free_one_fixed_converges_to_fixed() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(3.0, 4.0), true));
        s.set_constraints(vec![coincident(a, b)]);
        let r = s.solve();
        match r.status {
            SolverStatus::Converged { final_error, .. } => {
                assert!(final_error < 1e-8, "final_error={}", final_error);
            }
            other => panic!("expected Converged, got {:?}", other),
        }
    }

    #[test]
    fn loose_tolerance_converges_in_zero_iterations() {
        let mut s = ConstraintSolver::new();
        s.set_tolerance(1.0); // anything finite is "good enough"
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(0.5, 0.5), true));
        s.set_constraints(vec![coincident(a, b)]);
        let r = s.solve();
        if let SolverStatus::Converged { iterations, .. } = r.status {
            assert_eq!(iterations, 0);
        } // else under-constrained — also acceptable: nothing to do
    }

    #[test]
    fn over_constrained_emits_status() {
        // 1 free point (DOF=2) with 5 X-coordinate constraints (DOF removed = 5).
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(0.0, 0.0), false));
        let constraints: Vec<Constraint> = (0..5)
            .map(|_| {
                Constraint::new_dimensional(
                    DimensionalConstraint::XCoordinate(1.0),
                    vec![p],
                    ConstraintPriority::High,
                )
            })
            .collect();
        s.set_constraints(constraints);
        let r = s.solve();
        match r.status {
            SolverStatus::OverConstrained {
                conflicting_constraints,
            } => {
                assert_eq!(conflicting_constraints, 3);
            }
            other => panic!("expected OverConstrained, got {:?}", other),
        }
    }

    #[test]
    fn under_constrained_emits_status() {
        // 2 free points (DOF=4) with no constraints.
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 1.0), false));
        let r = s.solve();
        match r.status {
            SolverStatus::UnderConstrained { degrees_of_freedom } => {
                assert_eq!(degrees_of_freedom, 4);
            }
            other => panic!("expected UnderConstrained, got {:?}", other),
        }
    }

    #[test]
    fn fixed_point_has_zero_dof() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(1.0, 2.0), true));
        assert_eq!(s.count_degrees_of_freedom(), 0);
    }

    #[test]
    fn free_circle_has_three_dof() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 0.0), 1.0, false, false),
        );
        assert_eq!(s.count_degrees_of_freedom(), 3);
    }

    // ──────────────── C. Geometric-constraint evaluators ──────────────

    #[test]
    fn coincident_error_is_xy_difference() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(1.0, 2.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(4.0, 7.0), false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Coincident, &[a, b]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], -5.0, 1e-12));
    }

    #[test]
    fn coincident_zero_when_collocated() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(2.0, 3.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(2.0, 3.0), false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Coincident, &[a, b]);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
        assert!(approx_eq(errs[1], 0.0, 1e-12));
    }

    #[test]
    fn parallel_lines_error_zero_for_aligned_directions() {
        let mut s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        s.add_entity(
            l1,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 0.0), false, false),
        );
        s.add_entity(
            l2,
            EntityState::line(
                Point2d::new(3.0, 4.0),
                Vector2d::new(2.0, 0.0),
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Parallel, &[l1, l2]);
        assert_eq!(errs.len(), 1);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
    }

    #[test]
    fn perpendicular_lines_error_zero_for_orthogonal_directions() {
        let mut s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        s.add_entity(
            l1,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_X, false, false),
        );
        s.add_entity(
            l2,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_Y, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Perpendicular, &[l1, l2]);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
    }

    // ──────────── Angle constraint residual (KittyCAD/ezpz #244) ────────────
    // The residual must be SINGLE-VALUED — zero only at the target angle θ,
    // not at θ+π. A scalar cross/dot residual (`sin(Δ−θ)`) is zero at both
    // and lets a solve slip to the antiparallel branch (the bug ezpz fixed in
    // `PointsAtAngle`). We mirror their magnitude-scaled vector residual
    // `(|d1|+|d2|)/2 · (d2_hat − R(θ)·d1_hat)`.

    #[test]
    fn intersection_angle_residual_zero_at_target() {
        let mut s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        // d1 at 0°, d2 at 90°, target 90° → satisfied.
        s.add_entity(
            l1,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 0.0), false, false),
        );
        s.add_entity(
            l2,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(0.0, 1.0), false, false),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::IntersectionAngle(std::f64::consts::FRAC_PI_2),
            &[l1, l2],
        );
        assert_eq!(errs.len(), 2);
        assert!(
            approx_eq(errs[0], 0.0, 1e-12) && approx_eq(errs[1], 0.0, 1e-12),
            "{errs:?}"
        );
    }

    #[test]
    fn intersection_angle_rejects_antiparallel_branch() {
        // d1 at 0°, d2 at −90° (flipped), target +90°. The true signed
        // separation is −90°, NOT +90°, so the residual MUST be large. A
        // naive sin(Δ−θ) scalar reads sin(−180°)=0 here (false-satisfied);
        // the vector residual reads ‖(0,−2)‖ = 2.
        let mut s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        s.add_entity(
            l1,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 0.0), false, false),
        );
        s.add_entity(
            l2,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(0.0, -1.0), false, false),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::IntersectionAngle(std::f64::consts::FRAC_PI_2),
            &[l1, l2],
        );
        let norm = (errs[0] * errs[0] + errs[1] * errs[1]).sqrt();
        assert!(
            norm > 1.0,
            "antiparallel must not read as satisfied: {errs:?} (norm {norm})"
        );
    }

    #[test]
    fn dimensional_angle_residual_agrees_with_intersection_angle() {
        let mut s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        // Non-unit directions to exercise the magnitude scaling; d2 is at 45°.
        s.add_entity(
            l1,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(2.0, 0.0), false, false),
        );
        s.add_entity(
            l2,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 1.0), false, false),
        );
        let theta = std::f64::consts::FRAC_PI_4;
        let g = s.evaluate_geometric_constraint(
            &GeometricConstraint::IntersectionAngle(theta),
            &[l1, l2],
        );
        let d = s.evaluate_dimensional_constraint(&DimensionalConstraint::Angle(theta), &[l1, l2]);
        assert_eq!(g.len(), 2);
        assert_eq!(d.len(), 2);
        assert!(approx_eq(g[0], d[0], 1e-12) && approx_eq(g[1], d[1], 1e-12));
        // d1 at 0°, d2 at 45°, target 45° → satisfied.
        assert!(
            approx_eq(g[0], 0.0, 1e-12) && approx_eq(g[1], 0.0, 1e-12),
            "{g:?}"
        );
    }

    #[test]
    fn angle_constraint_error_count_matches_evaluator() {
        let s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        let ia = Constraint::new_geometric(
            GeometricConstraint::IntersectionAngle(1.0),
            vec![l1, l2],
            ConstraintPriority::Required,
        );
        let da = Constraint::new_dimensional(
            DimensionalConstraint::Angle(1.0),
            vec![l1, l2],
            ConstraintPriority::Required,
        );
        assert_eq!(
            s.constraint_error_count(&ia),
            s.evaluate_constraint_error(&ia).len()
        );
        assert_eq!(
            s.constraint_error_count(&da),
            s.evaluate_constraint_error(&da).len()
        );
        assert_eq!(s.constraint_error_count(&ia), 2);
        assert_eq!(s.constraint_error_count(&da), 2);
    }

    #[test]
    fn angle_constraint_solves_to_target_not_supplement() {
        // Free line nudged from ~6° toward a 60° angle against a pinned
        // reference line. A single-valued residual converges to 60° — not the
        // antiparallel 240° (which a cross/dot residual would also accept).
        let mut s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        s.add_entity(
            l1,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 0.0), true, true),
        );
        // Point pinned, direction free: the angle is the only free DOF.
        s.add_entity(
            l2,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 0.1), true, false),
        );
        let target = std::f64::consts::FRAC_PI_3; // 60°
        let c = Constraint::new_dimensional(
            DimensionalConstraint::Angle(target),
            vec![l1, l2],
            ConstraintPriority::Required,
        );
        s.set_constraints(vec![c]);
        let _ = s.solve();
        let d1 = s.get_line_direction(&l1).expect("l1 dir");
        let d2 = s.get_line_direction(&l2).expect("l2 dir");
        let solved = d1.signed_angle_to(&d2).expect("signed angle");
        assert!(
            approx_eq(solved, target, 1e-3),
            "solved angle {solved} rad, expected {target}"
        );
    }

    #[test]
    fn horizontal_error_is_dir_y() {
        let mut s = ConstraintSolver::new();
        let l = line_ref();
        s.add_entity(
            l,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(1.0, 0.5), false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Horizontal, &[l]);
        assert!(approx_eq(errs[0], 0.5, 1e-12));
    }

    #[test]
    fn vertical_error_is_dir_x() {
        let mut s = ConstraintSolver::new();
        let l = line_ref();
        s.add_entity(
            l,
            EntityState::line(Point2d::ORIGIN, Vector2d::new(0.25, 1.0), false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Vertical, &[l]);
        assert!(approx_eq(errs[0], 0.25, 1e-12));
    }

    #[test]
    fn tangent_line_circle_error_perp_distance_minus_radius() {
        // Line along x-axis through origin; circle at (0, 5), r = 3.
        // Perpendicular distance from circle center to line = 5; error = 5 - 3 = 2.
        let mut s = ConstraintSolver::new();
        let l = line_ref();
        let c = circle_ref();
        s.add_entity(
            l,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_X, false, false),
        );
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 5.0), 3.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Tangent, &[l, c]);
        assert!(approx_eq(errs[0], 2.0, 1e-12));
    }

    #[test]
    fn concentric_circles_error_is_center_diff() {
        let mut s = ConstraintSolver::new();
        let c1 = circle_ref();
        let c2 = circle_ref();
        s.add_entity(
            c1,
            EntityState::circle(Point2d::new(1.0, 2.0), 5.0, false, false),
        );
        s.add_entity(
            c2,
            EntityState::circle(Point2d::new(4.0, 6.0), 5.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Concentric, &[c1, c2]);
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], -4.0, 1e-12));
    }

    #[test]
    fn equal_circles_error_is_radius_diff() {
        let mut s = ConstraintSolver::new();
        let c1 = circle_ref();
        let c2 = circle_ref();
        s.add_entity(c1, EntityState::circle(Point2d::ORIGIN, 7.0, false, false));
        s.add_entity(c2, EntityState::circle(Point2d::ORIGIN, 4.0, false, false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Equal, &[c1, c2]);
        assert!(approx_eq(errs[0], 3.0, 1e-12));
    }

    #[test]
    fn point_on_line_zero_error_when_on_line() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let l = line_ref();
        s.add_entity(p, EntityState::point(Point2d::new(5.0, 0.0), false));
        s.add_entity(
            l,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_X, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, l]);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
    }

    #[test]
    fn point_on_line_nonzero_error_when_off_line() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let l = line_ref();
        s.add_entity(p, EntityState::point(Point2d::new(5.0, 3.0), false));
        s.add_entity(
            l,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_X, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, l]);
        assert!(errs[0].abs() > 1e-6);
    }

    #[test]
    fn point_on_circle_error_dist_minus_radius() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let c = circle_ref();
        s.add_entity(p, EntityState::point(Point2d::new(5.0, 0.0), false));
        s.add_entity(c, EntityState::circle(Point2d::ORIGIN, 3.0, false, false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, c]);
        assert!(approx_eq(errs[0], 2.0, 1e-12));
    }

    // PointOnCurve · B-Spline (slice C-4-a). The residual is the
    // signed perpendicular distance from the point to the spline's
    // closest-point foot, computed via `BSpline2d::closest_point` +
    // tangent dispatch. NURBS support is deferred to a sibling
    // slice that lands a native rational `closest_point` on
    // `NurbsCurve2d`.

    #[test]
    fn point_on_bspline_zero_error_when_on_curve() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let sp = spline_ref();
        // Clamped endpoint: the spline interpolates control_points[0]
        // = (0, 0), so a point exactly there sits on the curve.
        s.add_entity(p, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(sp, sample_bspline_state());
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, sp]);
        // closest_point's Newton refinement converges to a tight
        // residual; the foot snaps to (0, 0) and the perpendicular
        // distance is at numerical noise.
        assert!(errs[0].abs() < 1e-8);
    }

    #[test]
    fn point_on_bspline_nonzero_error_when_off_curve() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let sp = spline_ref();
        // (1.5, -3.0) sits well below the curve (which lies at y ≥ 0
        // across the entire parameter range). The signed residual
        // should be large in magnitude — the exact sign depends on
        // the tangent's CCW orientation at the foot, so assert only
        // on magnitude.
        s.add_entity(p, EntityState::point(Point2d::new(1.5, -3.0), false));
        s.add_entity(sp, sample_bspline_state());
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, sp]);
        assert!(
            errs[0].abs() > 1.0,
            "expected large off-curve residual, got {}",
            errs[0]
        );
    }

    #[test]
    fn point_on_bspline_emits_single_residual() {
        // PointOnCurve has a single-residual contract regardless of
        // curve kind — `constraint_error_count` returns 1 for every
        // non-Equal geometric constraint, so the spline arm must
        // not break that invariant.
        let s = ConstraintSolver::new();
        let p = point_ref();
        let sp = spline_ref();
        s.add_entity(p, EntityState::point(Point2d::new(1.5, -3.0), false));
        s.add_entity(sp, sample_bspline_state());
        let c = Constraint::new_geometric(
            GeometricConstraint::PointOnCurve,
            vec![p, sp],
            ConstraintPriority::High,
        );
        assert_eq!(s.constraint_error_count(&c), 1);
    }

    #[test]
    fn spline_bspline_dof_equals_two_per_control_point() {
        // Four control points × 2 free coords = 8 DOFs when nothing
        // is pinned. Matches `ParametricSpline2d::degrees_of_freedom`
        // for the non-rational case.
        let s = ConstraintSolver::new();
        let sp = spline_ref();
        s.add_entity(sp, sample_bspline_state());
        assert_eq!(s.count_degrees_of_freedom(), 8);
    }

    #[test]
    fn spline_bspline_fixed_flag_pins_all_control_points() {
        // Passing `fixed_control_points = true` to `spline_bspline`
        // pins every (x, y) pair, removing all DOFs the solver could
        // move. Matches the rectangle / ellipse "fix everything"
        // convention.
        let s = ConstraintSolver::new();
        let sp = spline_ref();
        let control_points = vec![
            Point2d::new(0.0, 0.0),
            Point2d::new(1.0, 0.0),
            Point2d::new(2.0, 0.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        s.add_entity(
            sp,
            EntityState::spline_bspline(2, control_points, knots, true),
        );
        assert_eq!(s.count_degrees_of_freedom(), 0);
    }

    #[test]
    fn spline_get_entity_updates_returns_parameters() {
        // The Spline arm in `get_entity_updates` already emits
        // `EntityUpdate::Parameters(state.parameters.clone())`
        // (pre-dates C-4-a). Pin that contract: the byte-for-byte
        // parameter layout must equal the constructor's pack order
        // so downstream consumers (sketch_solver) can unpack
        // control points by stride-2 indexing.
        let s = ConstraintSolver::new();
        let sp = spline_ref();
        let control_points = vec![
            Point2d::new(1.5, -2.5),
            Point2d::new(3.5, 4.5),
            Point2d::new(7.0, 0.25),
        ];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        s.add_entity(
            sp,
            EntityState::spline_bspline(2, control_points, knots, false),
        );
        let updates = s.get_entity_updates();
        let update = updates.get(&sp).expect("spline update missing");
        match update {
            EntityUpdate::Parameters(params) => {
                assert_eq!(params.len(), 6);
                assert!(approx_eq(params[0], 1.5, 1e-12));
                assert!(approx_eq(params[1], -2.5, 1e-12));
                assert!(approx_eq(params[2], 3.5, 1e-12));
                assert!(approx_eq(params[3], 4.5, 1e-12));
                assert!(approx_eq(params[4], 7.0, 1e-12));
                assert!(approx_eq(params[5], 0.25, 1e-12));
            }
            other => panic!("expected Parameters update, got {:?}", other),
        }
    }

    // PointOnCurve · NURBS (slice C-4-a-2). The dispatch lives in
    // `evaluate_point_on_curve`'s Spline arm: `is_spline_rational`
    // routes through `get_nurbs_curve2d` + `NurbsCurve2d::{closest_point,
    // tangent}` (native rational, not the lossy `to_bspline` route).
    // Tests use a quadratic NURBS quarter-arc — the rational weighting
    // is essential to keep the arc on the exact circle, which is what
    // distinguishes the rational path from the B-Spline path that
    // would project weights away.

    /// Build a quadratic NURBS quarter-arc state on the unit circle,
    /// centered at the origin, sweeping from `(1, 0)` to `(0, 1)`.
    /// Control points + weights are the standard one-segment NURBS arc
    /// construction (Piegl–Tiller §7.5): three control points with
    /// the corner point lifted to `(1, 1)` and middle weight `cos(45°)
    /// = √2/2`. The B-Spline projection of this curve (control points
    /// scaled by weights) is a parabola through `(1,0)`–`(√2/2, √2/2)`
    /// –`(0,1)`, which is NOT on the unit circle — the test fixture
    /// thereby separates the two code paths.
    fn sample_nurbs_quarter_arc_state() -> EntityState {
        let control_points = vec![
            Point2d::new(1.0, 0.0),
            Point2d::new(1.0, 1.0),
            Point2d::new(0.0, 1.0),
        ];
        let w = std::f64::consts::FRAC_1_SQRT_2;
        let weights = vec![1.0, w, 1.0];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        EntityState::spline_nurbs(2, control_points, weights, knots, false)
    }

    #[test]
    fn point_on_nurbs_zero_error_when_on_curve() {
        // The midpoint of the quarter arc sits at (√2/2, √2/2) on the
        // unit circle. A B-Spline using the same control points would
        // pass through (0.5, 0.5) at u = 0.5 (Bézier midpoint of an
        // (1,0)/(1,1)/(0,1) polygon), so getting near-zero residual at
        // (√2/2, √2/2) proves the rational dispatch fired.
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let sp = spline_ref();
        let half_sqrt2 = std::f64::consts::FRAC_1_SQRT_2;
        s.add_entity(
            p,
            EntityState::point(Point2d::new(half_sqrt2, half_sqrt2), false),
        );
        s.add_entity(sp, sample_nurbs_quarter_arc_state());
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, sp]);
        // closest_point's Newton refinement converges to a tight
        // residual on the arc.
        assert!(
            errs[0].abs() < 1e-6,
            "NURBS residual should be ~0 on the arc, got {}",
            errs[0]
        );
    }

    #[test]
    fn point_on_nurbs_nonzero_error_when_off_curve() {
        // (5, 5) is far outside the unit quarter arc — should produce
        // a large signed residual regardless of sign.
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let sp = spline_ref();
        s.add_entity(p, EntityState::point(Point2d::new(5.0, 5.0), false));
        s.add_entity(sp, sample_nurbs_quarter_arc_state());
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[p, sp]);
        assert!(
            errs[0].abs() > 1.0,
            "expected large off-curve NURBS residual, got {}",
            errs[0]
        );
    }

    #[test]
    fn nurbs_dispatch_distinct_from_bspline() {
        // Same control points, but one carries NURBS weights and the
        // other does not. At the midpoint (√2/2, √2/2) — on the
        // rational arc, not on the B-Spline parabola — the residuals
        // must differ in magnitude. This pins the dispatch: a regression
        // that routes NURBS through `to_bspline()` would make both
        // residuals match.
        let control_points = vec![
            Point2d::new(1.0, 0.0),
            Point2d::new(1.0, 1.0),
            Point2d::new(0.0, 1.0),
        ];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        let half_sqrt2 = std::f64::consts::FRAC_1_SQRT_2;
        let probe = Point2d::new(half_sqrt2, half_sqrt2);

        let mut s_bspline = ConstraintSolver::new();
        let pb = point_ref();
        let spb = spline_ref();
        s_bspline.add_entity(pb, EntityState::point(probe, false));
        s_bspline.add_entity(
            spb,
            EntityState::spline_bspline(2, control_points.clone(), knots.clone(), false),
        );
        let bs_err = s_bspline
            .evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[pb, spb])[0];

        let mut s_nurbs = ConstraintSolver::new();
        let pn = point_ref();
        let spn = spline_ref();
        s_nurbs.add_entity(pn, EntityState::point(probe, false));
        s_nurbs.add_entity(spn, sample_nurbs_quarter_arc_state());
        let nb_err = s_nurbs
            .evaluate_geometric_constraint(&GeometricConstraint::PointOnCurve, &[pn, spn])[0];

        // NURBS arc passes through the probe → ~0; B-Spline parabola
        // does not → ≥ 0.1. Magnitudes must differ enough that the
        // dispatch is unambiguous.
        assert!(
            (bs_err.abs() - nb_err.abs()).abs() > 0.05,
            "rational vs non-rational residual collapsed: bs={}, nb={}",
            bs_err,
            nb_err,
        );
    }

    #[test]
    fn spline_nurbs_dof_equals_two_per_control_point() {
        // Weights are pinned in metadata — DOF count matches the
        // B-Spline of equal control-point count. Three CPs × 2 free
        // coords = 6 DOFs.
        let s = ConstraintSolver::new();
        let sp = spline_ref();
        s.add_entity(sp, sample_nurbs_quarter_arc_state());
        assert_eq!(s.count_degrees_of_freedom(), 6);
    }

    #[test]
    fn spline_nurbs_fixed_flag_pins_all_control_points() {
        // Symmetric with the B-Spline case — `fixed_control_points =
        // true` zeroes every solver DOF; weights are already pinned
        // unconditionally.
        let s = ConstraintSolver::new();
        let sp = spline_ref();
        let control_points = vec![
            Point2d::new(1.0, 0.0),
            Point2d::new(1.0, 1.0),
            Point2d::new(0.0, 1.0),
        ];
        let weights = vec![1.0, std::f64::consts::FRAC_1_SQRT_2, 1.0];
        let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
        s.add_entity(
            sp,
            EntityState::spline_nurbs(2, control_points, weights, knots, true),
        );
        assert_eq!(s.count_degrees_of_freedom(), 0);
    }

    #[test]
    fn collinear_three_points_zero_for_aligned() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let c = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 1.0), false));
        s.add_entity(c, EntityState::point(Point2d::new(2.0, 2.0), false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Collinear, &[a, b, c]);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
    }

    #[test]
    fn collinear_three_points_nonzero_for_offset() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let c = point_ref();
        s.add_entity(a, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 0.0), false));
        s.add_entity(c, EntityState::point(Point2d::new(2.0, 1.0), false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Collinear, &[a, b, c]);
        assert!(errs[0].abs() > 0.5);
    }

    #[test]
    fn midpoint_constraint_zero_when_at_midpoint() {
        // Line goes from (0,0) along +x; get_line_end uses scale of 100,
        // so endpoint is (100,0); midpoint is (50, 0).
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let l = line_ref();
        s.add_entity(p, EntityState::point(Point2d::new(50.0, 0.0), false));
        s.add_entity(
            l,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_X, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Midpoint, &[p, l]);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
        assert!(approx_eq(errs[1], 0.0, 1e-12));
    }

    #[test]
    fn symmetric_zero_when_reflected_correctly() {
        // Axis line: through origin along +x. Reflection of (1, 1) across
        // x-axis is (1, -1).
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let axis = line_ref();
        s.add_entity(a, EntityState::point(Point2d::new(1.0, 1.0), false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, -1.0), false));
        s.add_entity(
            axis,
            EntityState::line(Point2d::ORIGIN, Vector2d::UNIT_X, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Symmetric, &[a, b, axis]);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
        assert!(approx_eq(errs[1], 0.0, 1e-12));
    }

    #[test]
    fn coincident_wrong_arity_returns_zeros() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        // Only one entity passed
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Coincident, &[p]);
        assert_eq!(errs, vec![0.0, 0.0]);
    }

    #[test]
    fn parallel_missing_entity_returns_zero() {
        // Both refs unknown to solver
        let s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Parallel, &[l1, l2]);
        // The solver returns Vector2d::UNIT_X for missing line directions,
        // so both directions match → cross product = 0.
        assert!(approx_eq(errs[0], 0.0, 1e-12));
    }

    // ─────────────── D. Dimensional-constraint evaluators ─────────────

    #[test]
    fn distance_error_is_actual_minus_target() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(b, EntityState::point(Point2d::new(3.0, 4.0), false));
        let errs =
            s.evaluate_dimensional_constraint(&DimensionalConstraint::Distance(2.0), &[a, b]);
        // Pythagorean distance is 5, target is 2 → error = 3
        assert!(approx_eq(errs[0], 3.0, 1e-12));
    }

    #[test]
    fn radius_error_is_actual_minus_target() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        s.add_entity(c, EntityState::circle(Point2d::ORIGIN, 5.0, false, false));
        let errs = s.evaluate_dimensional_constraint(&DimensionalConstraint::Radius(3.0), &[c]);
        assert!(approx_eq(errs[0], 2.0, 1e-12));
    }

    #[test]
    fn x_coordinate_error_is_pos_minus_target() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(1.5, 0.0), false));
        let errs =
            s.evaluate_dimensional_constraint(&DimensionalConstraint::XCoordinate(1.0), &[p]);
        assert!(approx_eq(errs[0], 0.5, 1e-12));
    }

    #[test]
    fn y_coordinate_error_is_pos_minus_target() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(0.0, -2.3), false));
        let errs =
            s.evaluate_dimensional_constraint(&DimensionalConstraint::YCoordinate(0.0), &[p]);
        assert!(approx_eq(errs[0], -2.3, 1e-12));
    }

    #[test]
    fn distance_missing_entity_returns_zero() {
        let s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let errs =
            s.evaluate_dimensional_constraint(&DimensionalConstraint::Distance(5.0), &[a, b]);
        assert_eq!(errs, vec![0.0]);
    }

    #[test]
    fn radius_for_non_circle_entity_returns_zero() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::ORIGIN, false));
        let errs = s.evaluate_dimensional_constraint(&DimensionalConstraint::Radius(1.0), &[p]);
        assert_eq!(errs, vec![0.0]);
    }

    // ───────────────── E. Jacobian / numerical differentiation ────────

    #[test]
    fn jacobian_dimensions_match_system_size() {
        // 1 free point (DOF=2) + 1 distance constraint (1 row).
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, true));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 1.0), false));
        s.set_constraints(vec![distance(a, b, 1.0)]);
        let j = s.compute_jacobian();
        assert_eq!(j.len(), 1, "rows = number of error components");
        assert_eq!(j[0].len(), 2, "cols = number of free parameters");
    }

    #[test]
    fn jacobian_skips_fixed_parameters() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        // Point is fully fixed → 0 free params.
        s.add_entity(p, EntityState::point(Point2d::new(1.0, 2.0), true));
        s.set_constraints(vec![Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(0.0),
            vec![p],
            ConstraintPriority::High,
        )]);
        let j = s.compute_jacobian();
        assert_eq!(j[0].len(), 0);
    }

    #[test]
    fn jacobian_restores_perturbed_parameters() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, true));
        s.add_entity(b, EntityState::point(Point2d::new(2.0, 3.0), false));
        s.set_constraints(vec![distance(a, b, 1.0)]);
        let _ = s.compute_jacobian();
        let entry = s.entity_state.get(&b).expect("b present");
        assert!(approx_eq(entry.parameters[0], 2.0, 1e-12));
        assert!(approx_eq(entry.parameters[1], 3.0, 1e-12));
    }

    #[test]
    fn jacobian_numerical_derivative_matches_distance() {
        // For points A=(0,0) fixed and B=(1,0) free, distance constraint
        // d(A,B)=t. ∂error/∂Bx = 1, ∂error/∂By = 0 at this configuration.
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, true));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 0.0), false));
        s.set_constraints(vec![distance(a, b, 0.5)]);
        let j = s.compute_jacobian();
        assert!(approx_eq(j[0][0], 1.0, 1e-5));
        assert!(approx_eq(j[0][1], 0.0, 1e-5));
    }

    #[test]
    fn constraint_error_count_matches_evaluator_output() {
        let s = ConstraintSolver::new();
        let coinc = coincident(point_ref(), point_ref());
        let dist = distance(point_ref(), point_ref(), 1.0);
        assert_eq!(s.constraint_error_count(&coinc), 2);
        assert_eq!(s.constraint_error_count(&dist), 1);
    }

    // ───────────────── E.5 Diagnose: redundancy & conflicts ───────────

    #[test]
    fn jacobian_row_owners_maps_each_row_to_its_constraint() {
        // Coincident contributes 2 rows; Distance contributes 1. The
        // first two entries of the owner map should equal the first
        // constraint's id; the third entry should equal the second's.
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 0.0), false));
        let c1 = coincident(a, b);
        let c2 = distance(a, b, 1.0);
        let id1 = c1.id;
        let id2 = c2.id;
        s.set_constraints(vec![c1, c2]);
        let owners = s.jacobian_row_owners();
        assert_eq!(owners.len(), 3);
        assert_eq!(owners[0], id1);
        assert_eq!(owners[1], id1);
        assert_eq!(owners[2], id2);
    }

    #[test]
    fn diagnose_empty_solver_returns_default() {
        let s = ConstraintSolver::new();
        let d = s.diagnose();
        assert!(d.redundant.is_empty());
        assert!(d.conflicts.is_empty());
        assert_eq!(d.jacobian_rank, 0);
        assert_eq!(d.jacobian_rows, 0);
    }

    #[test]
    fn diagnose_marks_no_constraint_redundant_when_all_independent() {
        // Free point + XCoordinate + YCoordinate: 2 rows, full rank 2,
        // no constraint is dependent.
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(3.0, 4.0), false));
        let cx = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![p],
            ConstraintPriority::Required,
        );
        let cy = Constraint::new_dimensional(
            DimensionalConstraint::YCoordinate(4.0),
            vec![p],
            ConstraintPriority::Required,
        );
        s.set_constraints(vec![cx, cy]);
        let d = s.diagnose();
        assert!(d.redundant.is_empty(), "expected no redundant: {:?}", d);
        assert!(d.conflicts.is_empty(), "expected no conflicts: {:?}", d);
        assert_eq!(d.jacobian_rank, 2);
        assert_eq!(d.jacobian_rows, 2);
    }

    #[test]
    fn diagnose_classifies_duplicate_x_constraint_as_redundant() {
        // Two XCoordinate(3.0) on the same point: identical Jacobian
        // rows AND identical RHS, residual zero → redundant.
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(3.0, 4.0), false));
        let cx_first = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![p],
            ConstraintPriority::Required,
        );
        let cx_dup = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![p],
            ConstraintPriority::Required,
        );
        let dup_id = cx_dup.id;
        s.set_constraints(vec![cx_first, cx_dup]);
        let d = s.diagnose();
        assert_eq!(d.redundant, vec![dup_id]);
        assert!(d.conflicts.is_empty());
        assert_eq!(d.jacobian_rank, 1);
        assert_eq!(d.jacobian_rows, 2);
    }

    #[test]
    fn diagnose_classifies_contradictory_x_constraint_as_conflict() {
        // Two XCoordinate constraints with different targets:
        // identical Jacobian rows but inconsistent RHS. The point's
        // current position satisfies the first but not the second
        // (or vice-versa) → second row dependent + non-zero residual
        // → conflict.
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(3.0, 4.0), false));
        let cx_first = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![p],
            ConstraintPriority::Required,
        );
        let cx_conflict = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(7.0),
            vec![p],
            ConstraintPriority::Required,
        );
        let conflict_id = cx_conflict.id;
        s.set_constraints(vec![cx_first, cx_conflict]);
        let d = s.diagnose();
        assert!(d.redundant.is_empty(), "got redundant: {:?}", d.redundant);
        assert_eq!(d.conflicts, vec![conflict_id]);
        assert_eq!(d.jacobian_rank, 1);
    }

    #[test]
    fn diagnose_preserves_essential_constraint_when_rank_full() {
        // 1 free point + Distance from a fixed origin + XCoordinate:
        // 2 rows on a 2-DOF system. Both independent at the current
        // configuration (Distance row has both x and y partials;
        // XCoordinate row has only x). No constraint is dependent.
        let mut s = ConstraintSolver::new();
        let origin = point_ref();
        let p = point_ref();
        s.add_entity(origin, EntityState::point(Point2d::ORIGIN, true));
        s.add_entity(p, EntityState::point(Point2d::new(3.0, 4.0), false));
        let cd = distance(origin, p, 5.0);
        let cx = Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![p],
            ConstraintPriority::Required,
        );
        s.set_constraints(vec![cd, cx]);
        let d = s.diagnose();
        assert!(d.redundant.is_empty());
        assert!(d.conflicts.is_empty());
        assert_eq!(d.jacobian_rank, 2);
    }

    // ─────────────────── F. Gaussian elimination ──────────────────────

    #[test]
    fn gauss_solves_identity() {
        let s = ConstraintSolver::new();
        let a = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
        ];
        let b = vec![1.0, 2.0, 3.0];
        let x = s.gaussian_elimination(a, b).expect("ok");
        assert!(approx_eq(x[0], 1.0, 1e-12));
        assert!(approx_eq(x[1], 2.0, 1e-12));
        assert!(approx_eq(x[2], 3.0, 1e-12));
    }

    #[test]
    fn gauss_solves_2x2_simple() {
        let s = ConstraintSolver::new();
        // [[2,1],[1,2]] * [1,1]^T = [3,3]
        let a = vec![vec![2.0, 1.0], vec![1.0, 2.0]];
        let b = vec![3.0, 3.0];
        let x = s.gaussian_elimination(a, b).expect("ok");
        assert!(approx_eq(x[0], 1.0, 1e-12));
        assert!(approx_eq(x[1], 1.0, 1e-12));
    }

    #[test]
    fn gauss_pivots_when_first_row_has_zero_diag() {
        let s = ConstraintSolver::new();
        // [[0,1],[1,0]] * [x,y]^T = [2,3] → x=3, y=2
        let a = vec![vec![0.0, 1.0], vec![1.0, 0.0]];
        let b = vec![2.0, 3.0];
        let x = s.gaussian_elimination(a, b).expect("ok");
        assert!(approx_eq(x[0], 3.0, 1e-12));
        assert!(approx_eq(x[1], 2.0, 1e-12));
    }

    #[test]
    fn gauss_singular_matrix_returns_err() {
        let s = ConstraintSolver::new();
        // Linearly dependent rows
        let a = vec![vec![1.0, 2.0], vec![2.0, 4.0]];
        let b = vec![3.0, 6.0];
        assert!(s.gaussian_elimination(a, b).is_err());
    }

    #[test]
    fn gauss_handles_3x3_with_pivoting() {
        let s = ConstraintSolver::new();
        // System: x+y+z=6, 2y+5z=-4, 2x+5y-z=27 → x=5, y=3, z=-2
        let a = vec![
            vec![1.0, 1.0, 1.0],
            vec![0.0, 2.0, 5.0],
            vec![2.0, 5.0, -1.0],
        ];
        let b = vec![6.0, -4.0, 27.0];
        let x = s.gaussian_elimination(a, b).expect("ok");
        assert!(approx_eq(x[0], 5.0, 1e-9));
        assert!(approx_eq(x[1], 3.0, 1e-9));
        assert!(approx_eq(x[2], -2.0, 1e-9));
    }

    #[test]
    fn linear_solver_handles_empty_system() {
        let s = ConstraintSolver::new();
        let j: Vec<Vec<f64>> = vec![vec![]];
        let errs: Vec<f64> = vec![];
        let result = s.solve_linear_system(&j, &errs).expect("ok");
        assert!(result.is_empty());
    }

    #[test]
    fn linear_solver_least_squares_underdetermined() {
        // 1 equation, 2 unknowns: J = [[1, 1]], errors = [2].
        // `J^T·J = [[1,1],[1,1]]` is singular, so plain Gaussian
        // elimination cannot solve it — but the linear solver now
        // falls back to Tikhonov regularisation `(J^T·J + λI)·dx
        // = -J^T·r`, which produces the well-defined minimum-norm
        // solution `dx = (-1, -1)`. This is exactly the desired
        // behaviour for under-constrained sketch updates: advance
        // the parameters by the smallest move that satisfies the
        // active constraint.
        let s = ConstraintSolver::new();
        let j = vec![vec![1.0, 1.0]];
        let errs = vec![2.0];
        let dx = s
            .solve_linear_system(&j, &errs)
            .expect("Tikhonov fallback solves singular normal equations");
        // λ is small (trace-scaled, 1e-8 here), so the minimum-norm
        // solution is reproduced to ~1e-8 absolute.
        assert!((dx[0] - (-1.0)).abs() < 1e-7, "dx[0] = {}", dx[0]);
        assert!((dx[1] - (-1.0)).abs() < 1e-7, "dx[1] = {}", dx[1]);
    }

    // ────────────────── G. apply_updates / damping ────────────────────

    #[test]
    fn apply_updates_with_default_damping_half() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(10.0, 20.0), false));
        s.apply_updates(&[2.0, 4.0], 0.5);
        let entry = s.entity_state.get(&p).expect("present");
        assert!(approx_eq(entry.parameters[0], 11.0, 1e-12));
        assert!(approx_eq(entry.parameters[1], 22.0, 1e-12));
    }

    #[test]
    fn apply_updates_zero_damping_freezes_state() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(7.0, 8.0), false));
        s.apply_updates(&[100.0, 200.0], 0.0);
        let entry = s.entity_state.get(&p).expect("present");
        assert_eq!(entry.parameters[0], 7.0);
        assert_eq!(entry.parameters[1], 8.0);
    }

    #[test]
    fn apply_updates_full_damping_takes_full_step() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.apply_updates(&[3.0, 4.0], 1.0);
        let entry = s.entity_state.get(&p).expect("present");
        assert!(approx_eq(entry.parameters[0], 3.0, 1e-12));
        assert!(approx_eq(entry.parameters[1], 4.0, 1e-12));
    }

    #[test]
    fn apply_updates_skips_fixed_components() {
        // Line with point fixed and direction free.
        let s = ConstraintSolver::new();
        let l = line_ref();
        s.add_entity(
            l,
            EntityState::line(Point2d::new(1.0, 1.0), Vector2d::new(0.0, 0.0), true, false),
        );
        // 2 free params (dx, dy); apply [10, 20] with full damping.
        s.apply_updates(&[10.0, 20.0], 1.0);
        let entry = s.entity_state.get(&l).expect("present");
        assert_eq!(entry.parameters[0], 1.0); // point.x untouched
        assert_eq!(entry.parameters[1], 1.0); // point.y untouched
        assert!(approx_eq(entry.parameters[2], 10.0, 1e-12));
        assert!(approx_eq(entry.parameters[3], 20.0, 1e-12));
    }

    // ─────────────────── H. Violation reporting ───────────────────────

    #[test]
    fn violations_empty_when_constraints_satisfied() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(b, EntityState::point(Point2d::ORIGIN, false));
        s.set_constraints(vec![coincident(a, b)]);
        let v = s.get_violations();
        assert!(v.is_empty());
    }

    #[test]
    fn violations_populated_when_above_tolerance() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(b, EntityState::point(Point2d::new(3.0, 4.0), false));
        let c = coincident(a, b);
        let cid = c.id;
        s.set_constraints(vec![c]);
        let v = s.get_violations();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].0, cid);
        assert!(approx_eq(v[0].1, 5.0, 1e-12)); // sqrt(3² + 4²)
    }

    #[test]
    fn violations_filtered_below_tolerance() {
        let mut s = ConstraintSolver::new();
        s.set_tolerance(10.0);
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 0.0), false));
        s.set_constraints(vec![coincident(a, b)]);
        let v = s.get_violations();
        assert!(v.is_empty());
    }

    // ────────────────── I. Robustness / edge cases ────────────────────

    #[test]
    fn get_entity_updates_returns_point_variant_for_point_ref() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(3.0, 4.0), false));
        let updates = s.get_entity_updates();
        match updates.get(&p).expect("present") {
            EntityUpdate::Point(pt) => {
                assert_eq!(pt.x, 3.0);
                assert_eq!(pt.y, 4.0);
            }
            other => panic!("expected Point variant, got {:?}", other),
        }
    }

    #[test]
    fn get_entity_updates_returns_line_variant_for_line_ref() {
        let s = ConstraintSolver::new();
        let l = line_ref();
        s.add_entity(
            l,
            EntityState::line(
                Point2d::new(1.0, 2.0),
                Vector2d::new(3.0, 4.0),
                false,
                false,
            ),
        );
        let updates = s.get_entity_updates();
        match updates.get(&l).expect("present") {
            EntityUpdate::Line(pt, dir) => {
                assert_eq!(pt.x, 1.0);
                assert_eq!(pt.y, 2.0);
                assert_eq!(dir.x, 3.0);
                assert_eq!(dir.y, 4.0);
            }
            other => panic!("expected Line variant, got {:?}", other),
        }
    }

    #[test]
    fn get_entity_updates_returns_circle_variant_for_circle_ref() {
        let s = ConstraintSolver::new();
        let c = circle_ref();
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(5.0, 6.0), 2.5, false, false),
        );
        let updates = s.get_entity_updates();
        match updates.get(&c).expect("present") {
            EntityUpdate::Circle(center, r) => {
                assert_eq!(center.x, 5.0);
                assert_eq!(center.y, 6.0);
                assert_eq!(*r, 2.5);
            }
            other => panic!("expected Circle variant, got {:?}", other),
        }
    }

    #[test]
    fn dependency_graph_rebuilt_on_set_constraints() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let c = point_ref();
        let first = coincident(a, b);
        let second = coincident(b, c);
        s.set_constraints(vec![first.clone()]);
        assert_eq!(s.dependency_graph.len(), 1);
        s.set_constraints(vec![second.clone()]);
        // Old entry is gone; new one is present.
        assert_eq!(s.dependency_graph.len(), 1);
        assert!(!s.dependency_graph.contains_key(&first.id));
        assert!(s.dependency_graph.contains_key(&second.id));
    }

    #[test]
    fn missing_point_in_coincident_yields_zero_error() {
        // entities[0] missing → get_point_position returns None → falls
        // through to the `(None, _)` arm, which yields `vec![0.0, 0.0]`.
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 2.0), false));
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Coincident, &[a, b]);
        assert_eq!(errs, vec![0.0, 0.0]);
    }

    #[test]
    fn solve_empty_returns_finite_solve_time() {
        let mut s = ConstraintSolver::new();
        let r = s.solve();
        assert!(r.solve_time_ms.is_finite());
        assert!(r.solve_time_ms >= 0.0);
    }

    #[test]
    fn count_constraint_dof_aggregates_priorities() {
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        // Coincident removes 2 DOF, distance removes 1 DOF → 3 total.
        s.set_constraints(vec![coincident(a, b), distance(a, b, 1.0)]);
        assert_eq!(s.count_constraint_dof(), 3);
    }

    #[test]
    fn x_coordinate_constraint_drives_solver_to_target() {
        // Single free point; one X-coordinate constraint (DOF=2 vs 1).
        // System is under-constrained; solver returns UnderConstrained
        // without iterating, regardless of input x.
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(7.0, 0.0), false));
        s.set_constraints(vec![Constraint::new_dimensional(
            DimensionalConstraint::XCoordinate(3.0),
            vec![p],
            ConstraintPriority::High,
        )]);
        let r = s.solve();
        assert!(matches!(r.status, SolverStatus::UnderConstrained { .. }));
    }

    #[test]
    fn fully_constrained_xy_drives_point_to_target() {
        // 1 free point (2 DOF) + 2 dimensional constraints (X + Y).
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(0.0, 0.0), false));
        s.set_constraints(vec![
            Constraint::new_dimensional(
                DimensionalConstraint::XCoordinate(3.0),
                vec![p],
                ConstraintPriority::High,
            ),
            Constraint::new_dimensional(
                DimensionalConstraint::YCoordinate(4.0),
                vec![p],
                ConstraintPriority::High,
            ),
        ]);
        let r = s.solve();
        match r.status {
            SolverStatus::Converged { final_error, .. } => {
                assert!(final_error < 1e-8, "final_error = {}", final_error);
            }
            other => panic!("expected Converged, got {:?}", other),
        }
        // Verify the point landed at (3, 4).
        match r.entity_updates.get(&p).expect("update for p present") {
            EntityUpdate::Point(pt) => {
                assert!(approx_eq(pt.x, 3.0, 1e-6));
                assert!(approx_eq(pt.y, 4.0, 1e-6));
            }
            other => panic!("expected Point update, got {:?}", other),
        }
    }

    #[test]
    fn set_constraints_sorts_by_priority_during_solve() {
        // Required (0) < High (1). solve() sorts ascending so that
        // higher-priority (lower-valued) constraints are processed first.
        // Use two free points + two constraints sized so check_constraint_count
        // returns None (4 DOF == 4 DOF removed) and the sort actually runs.
        let mut s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        s.add_entity(a, EntityState::point(Point2d::ORIGIN, false));
        s.add_entity(b, EntityState::point(Point2d::new(1.0, 0.0), false));
        let high = Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![a, b],
            ConstraintPriority::High,
        );
        let required = Constraint::new_geometric(
            GeometricConstraint::Coincident,
            vec![a, b],
            ConstraintPriority::Required,
        );
        s.set_constraints(vec![high, required]);
        let _ = s.solve();
        // After solve, constraints[0] should be Required.
        assert_eq!(s.constraints[0].priority, ConstraintPriority::Required);
        assert_eq!(s.constraints[1].priority, ConstraintPriority::High);
    }

    // EntityState constructors

    #[test]
    fn entity_state_point_layout() {
        let st = EntityState::point(Point2d::new(1.5, -2.5), false);
        assert_eq!(st.parameters, vec![1.5, -2.5]);
        assert_eq!(st.fixed_mask, vec![false, false]);
    }

    #[test]
    fn entity_state_point_fixed_layout() {
        let st = EntityState::point(Point2d::new(0.0, 0.0), true);
        assert_eq!(st.fixed_mask, vec![true, true]);
    }

    #[test]
    fn entity_state_line_layout() {
        let st = EntityState::line(Point2d::new(1.0, 2.0), Vector2d::new(3.0, 4.0), false, true);
        assert_eq!(st.parameters, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(st.fixed_mask, vec![false, false, true, true]);
    }

    #[test]
    fn entity_state_circle_layout() {
        let st = EntityState::circle(Point2d::new(5.0, 6.0), 7.0, true, false);
        assert_eq!(st.parameters, vec![5.0, 6.0, 7.0]);
        assert_eq!(st.fixed_mask, vec![true, true, false]);
    }

    // ── C-2: Rectangle entity ─────────────────────────────────────

    #[test]
    fn entity_state_rectangle_layout() {
        // Parameter order is [center.x, center.y, width, height,
        // rotation] so that params[0..2] aligns with the generic
        // `get_point_position` dispatch (which reads slot 0/1 from
        // any entity it understands).
        let st = EntityState::rectangle(
            Point2d::new(1.0, 2.0),
            3.0,
            4.0,
            std::f64::consts::FRAC_PI_4,
            false,
            true,
            false,
            true,
        );
        assert_eq!(
            st.parameters,
            vec![1.0, 2.0, 3.0, 4.0, std::f64::consts::FRAC_PI_4]
        );
        // `center_fixed = false` flows into BOTH x and y slots.
        assert_eq!(st.fixed_mask, vec![false, false, true, false, true]);
    }

    #[test]
    fn entity_state_rectangle_fixed_center_pins_both_axes() {
        let st = EntityState::rectangle(
            Point2d::new(0.0, 0.0),
            1.0,
            1.0,
            0.0,
            true,
            false,
            false,
            false,
        );
        assert_eq!(st.fixed_mask, vec![true, true, false, false, false]);
    }

    #[test]
    fn equal_rectangles_error_is_width_and_height_diff() {
        // Equal(Rectangle, Rectangle) is a 2-residual constraint:
        // r0 = w1 - w2, r1 = h1 - h2. Rotation is independent.
        let mut s = ConstraintSolver::new();
        let r1 = rect_ref();
        let r2 = rect_ref();
        s.add_entity(
            r1,
            EntityState::rectangle(Point2d::ORIGIN, 5.0, 3.0, 0.0, false, false, false, false),
        );
        s.add_entity(
            r2,
            EntityState::rectangle(Point2d::ORIGIN, 2.0, 7.0, 0.0, false, false, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Equal, &[r1, r2]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], 3.0, 1e-12));
        assert!(approx_eq(errs[1], -4.0, 1e-12));
    }

    #[test]
    fn coincident_rectangles_error_is_center_diff() {
        // get_point_position must read rectangle params[0..2] as
        // (center.x, center.y) so Coincident over two rectangles
        // measures center-to-center displacement.
        let mut s = ConstraintSolver::new();
        let r1 = rect_ref();
        let r2 = rect_ref();
        s.add_entity(
            r1,
            EntityState::rectangle(
                Point2d::new(1.0, 2.0),
                1.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        s.add_entity(
            r2,
            EntityState::rectangle(
                Point2d::new(4.0, -1.0),
                1.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Coincident, &[r1, r2]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], 3.0, 1e-12));
    }

    #[test]
    fn concentric_rectangles_error_is_center_diff() {
        // get_circle_center treats Rectangle alongside Circle/Arc so
        // Concentric works between any pair of (Circle, Arc, Rectangle).
        let mut s = ConstraintSolver::new();
        let r1 = rect_ref();
        let r2 = rect_ref();
        s.add_entity(
            r1,
            EntityState::rectangle(
                Point2d::new(2.0, 5.0),
                1.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        s.add_entity(
            r2,
            EntityState::rectangle(
                Point2d::new(2.0, 5.0),
                4.0,
                7.0,
                1.0,
                false,
                false,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Concentric, &[r1, r2]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
        assert!(approx_eq(errs[1], 0.0, 1e-12));
    }

    #[test]
    fn concentric_rectangle_with_circle_uses_centers() {
        let mut s = ConstraintSolver::new();
        let rect = rect_ref();
        let circ = circle_ref();
        s.add_entity(
            rect,
            EntityState::rectangle(
                Point2d::new(1.0, 1.0),
                2.0,
                2.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        s.add_entity(
            circ,
            EntityState::circle(Point2d::new(4.0, 5.0), 1.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Concentric, &[rect, circ]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], -4.0, 1e-12));
    }

    #[test]
    fn get_entity_updates_returns_rectangle_variant_for_rect_ref() {
        let s = ConstraintSolver::new();
        let r = rect_ref();
        s.add_entity(
            r,
            EntityState::rectangle(
                Point2d::new(1.5, 2.5),
                3.5,
                4.5,
                std::f64::consts::FRAC_PI_3,
                false,
                false,
                false,
                false,
            ),
        );
        let updates = s.get_entity_updates();
        match updates.get(&r).expect("present") {
            EntityUpdate::Rectangle(center, w, h, rot) => {
                assert_eq!(center.x, 1.5);
                assert_eq!(center.y, 2.5);
                assert_eq!(*w, 3.5);
                assert_eq!(*h, 4.5);
                assert!(approx_eq(*rot, std::f64::consts::FRAC_PI_3, 1e-12));
            }
            other => panic!("expected Rectangle variant, got {:?}", other),
        }
    }

    #[test]
    fn constraint_error_count_equal_two_rectangles_is_two() {
        // The dimension of the residual for Equal depends on the
        // entity kinds it ranges over. For two rectangles the
        // residual is (Δw, Δh) → 2 rows; the Jacobian must size
        // itself accordingly or the solver will silently drop or
        // overcount rows.
        let mut s = ConstraintSolver::new();
        let r1 = rect_ref();
        let r2 = rect_ref();
        s.add_entity(
            r1,
            EntityState::rectangle(Point2d::ORIGIN, 1.0, 1.0, 0.0, false, false, false, false),
        );
        s.add_entity(
            r2,
            EntityState::rectangle(Point2d::ORIGIN, 2.0, 2.0, 0.0, false, false, false, false),
        );
        let c = Constraint::new_geometric(
            GeometricConstraint::Equal,
            vec![r1, r2],
            ConstraintPriority::High,
        );
        assert_eq!(s.constraint_error_count(&c), 2);
    }

    #[test]
    fn constraint_error_count_equal_two_circles_is_one() {
        // Sanity counter-check: Equal over two circles is a single
        // radius residual. The rectangle-specific branch must not
        // bleed into other kinds.
        let mut s = ConstraintSolver::new();
        let c1 = circle_ref();
        let c2 = circle_ref();
        s.add_entity(c1, EntityState::circle(Point2d::ORIGIN, 1.0, false, false));
        s.add_entity(c2, EntityState::circle(Point2d::ORIGIN, 2.0, false, false));
        let c = Constraint::new_geometric(
            GeometricConstraint::Equal,
            vec![c1, c2],
            ConstraintPriority::High,
        );
        assert_eq!(s.constraint_error_count(&c), 1);
    }

    // ── C-3: Ellipse entity ───────────────────────────────────────

    #[test]
    fn entity_state_ellipse_layout() {
        // Parameter order is [center.x, center.y, semi_major,
        // semi_minor, rotation] so that params[0..2] aligns with the
        // generic `get_point_position` dispatch (which reads slot 0/1
        // from any entity it understands).
        let st = EntityState::ellipse(
            Point2d::new(1.0, 2.0),
            5.0,
            3.0,
            std::f64::consts::FRAC_PI_6,
            false,
            true,
            false,
            true,
        );
        assert_eq!(
            st.parameters,
            vec![1.0, 2.0, 5.0, 3.0, std::f64::consts::FRAC_PI_6]
        );
        // `center_fixed = false` flows into BOTH x and y slots.
        assert_eq!(st.fixed_mask, vec![false, false, true, false, true]);
    }

    #[test]
    fn entity_state_ellipse_fixed_center_pins_both_axes() {
        let st = EntityState::ellipse(
            Point2d::new(0.0, 0.0),
            2.0,
            1.0,
            0.0,
            true,
            false,
            false,
            false,
        );
        assert_eq!(st.fixed_mask, vec![true, true, false, false, false]);
    }

    #[test]
    fn equal_ellipses_error_is_axes_diff() {
        // Equal(Ellipse, Ellipse) is a 2-residual constraint:
        // r0 = a1 - a2, r1 = b1 - b2. Rotation is independent — two
        // ellipses of identical shape but different orientation are
        // still "equal", matching the rectangle convention.
        let mut s = ConstraintSolver::new();
        let e1 = ellipse_ref();
        let e2 = ellipse_ref();
        s.add_entity(
            e1,
            EntityState::ellipse(Point2d::ORIGIN, 5.0, 3.0, 0.0, false, false, false, false),
        );
        s.add_entity(
            e2,
            EntityState::ellipse(Point2d::ORIGIN, 2.0, 7.0, 0.0, false, false, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Equal, &[e1, e2]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], 3.0, 1e-12));
        assert!(approx_eq(errs[1], -4.0, 1e-12));
    }

    #[test]
    fn coincident_ellipses_error_is_center_diff() {
        // get_point_position must read ellipse params[0..2] as
        // (center.x, center.y) so Coincident over two ellipses
        // measures center-to-center displacement.
        let mut s = ConstraintSolver::new();
        let e1 = ellipse_ref();
        let e2 = ellipse_ref();
        s.add_entity(
            e1,
            EntityState::ellipse(
                Point2d::new(1.0, 2.0),
                2.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        s.add_entity(
            e2,
            EntityState::ellipse(
                Point2d::new(4.0, -1.0),
                2.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Coincident, &[e1, e2]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], 3.0, 1e-12));
    }

    #[test]
    fn concentric_ellipses_error_is_center_diff() {
        // get_circle_center treats Ellipse alongside Circle/Arc/
        // Rectangle so Concentric works between any pair.
        let mut s = ConstraintSolver::new();
        let e1 = ellipse_ref();
        let e2 = ellipse_ref();
        s.add_entity(
            e1,
            EntityState::ellipse(
                Point2d::new(2.0, 5.0),
                2.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        s.add_entity(
            e2,
            EntityState::ellipse(
                Point2d::new(2.0, 5.0),
                4.0,
                3.0,
                1.0,
                false,
                false,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Concentric, &[e1, e2]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], 0.0, 1e-12));
        assert!(approx_eq(errs[1], 0.0, 1e-12));
    }

    #[test]
    fn concentric_ellipse_with_circle_uses_centers() {
        // Mixed-kind Concentric: the centre dispatch unifies
        // Circle/Arc/Rectangle/Ellipse on params[0..2].
        let mut s = ConstraintSolver::new();
        let ell = ellipse_ref();
        let circ = circle_ref();
        s.add_entity(
            ell,
            EntityState::ellipse(
                Point2d::new(1.0, 1.0),
                2.0,
                1.0,
                0.0,
                false,
                false,
                false,
                false,
            ),
        );
        s.add_entity(
            circ,
            EntityState::circle(Point2d::new(4.0, 5.0), 1.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(&GeometricConstraint::Concentric, &[ell, circ]);
        assert_eq!(errs.len(), 2);
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], -4.0, 1e-12));
    }

    #[test]
    fn get_entity_updates_returns_ellipse_variant_for_ellipse_ref() {
        let s = ConstraintSolver::new();
        let e = ellipse_ref();
        s.add_entity(
            e,
            EntityState::ellipse(
                Point2d::new(1.5, 2.5),
                4.5,
                3.5,
                std::f64::consts::FRAC_PI_3,
                false,
                false,
                false,
                false,
            ),
        );
        let updates = s.get_entity_updates();
        match updates.get(&e).expect("present") {
            EntityUpdate::Ellipse(center, a, b, rot) => {
                assert_eq!(center.x, 1.5);
                assert_eq!(center.y, 2.5);
                assert_eq!(*a, 4.5);
                assert_eq!(*b, 3.5);
                assert!(approx_eq(*rot, std::f64::consts::FRAC_PI_3, 1e-12));
            }
            other => panic!("expected Ellipse variant, got {:?}", other),
        }
    }

    #[test]
    fn constraint_error_count_equal_two_ellipses_is_two() {
        // The dimension of the residual for Equal depends on the
        // entity kinds it ranges over. For two ellipses the residual
        // is (Δa, Δb) → 2 rows; the Jacobian must size itself
        // accordingly or the solver will silently drop or overcount
        // rows.
        let mut s = ConstraintSolver::new();
        let e1 = ellipse_ref();
        let e2 = ellipse_ref();
        s.add_entity(
            e1,
            EntityState::ellipse(Point2d::ORIGIN, 2.0, 1.0, 0.0, false, false, false, false),
        );
        s.add_entity(
            e2,
            EntityState::ellipse(Point2d::ORIGIN, 3.0, 2.0, 0.0, false, false, false, false),
        );
        let c = Constraint::new_geometric(
            GeometricConstraint::Equal,
            vec![e1, e2],
            ConstraintPriority::High,
        );
        assert_eq!(s.constraint_error_count(&c), 2);
    }

    #[test]
    fn solver_result_carries_status_constraint_type_arms() {
        // Just verifies all SolverStatus variants exist and are debug-printable.
        let _converged = SolverStatus::Converged {
            iterations: 1,
            final_error: 0.0,
        };
        let _not = SolverStatus::NotConverged {
            iterations: 99,
            final_error: 1.0,
        };
        let _over = SolverStatus::OverConstrained {
            conflicting_constraints: 1,
        };
        let _under = SolverStatus::UnderConstrained {
            degrees_of_freedom: 1,
        };
        let _unstable = SolverStatus::Unstable;
        let _ct = ConstraintType::Geometric(GeometricConstraint::Coincident);
    }
}
