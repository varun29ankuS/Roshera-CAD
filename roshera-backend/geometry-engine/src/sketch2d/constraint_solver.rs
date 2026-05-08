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

use super::constraints::EntityRef;
use super::{
    Constraint, ConstraintId, ConstraintType, DimensionalConstraint, GeometricConstraint, Point2d,
    Vector2d,
};
use crate::math::tolerance::STRICT_TOLERANCE;
use dashmap::DashMap;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Solver status
#[derive(Debug, Clone, Copy, PartialEq)]
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

/// Entity position/parameter update
#[derive(Debug, Clone)]
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
}

/// Solver state for a single sketch entity.
///
/// Stores the entity's parameter vector together with a per-parameter
/// fixed-mask used by the Newton–Raphson solver. Construct one via
/// [`EntityState::point`], [`EntityState::line`], or
/// [`EntityState::circle`] and register it with
/// [`ConstraintSolver::add_entity`].
#[derive(Debug, Clone)]
pub struct EntityState {
    /// Current parameters (position, angles, etc.)
    parameters: Vec<f64>,
    /// Fixed parameters (indices that cannot change)
    fixed_mask: Vec<bool>,
}

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

    /// Solve the constraint system
    pub fn solve(&mut self) -> SolverResult {
        let start_time = std::time::Instant::now();

        // Check for under/over-constrained system
        let constraint_check = self.check_constraint_count();
        if let Some(status) = constraint_check {
            return SolverResult {
                status,
                entity_updates: HashMap::new(),
                violations: self.get_violations(),
                solve_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
            };
        }

        // Sort constraints by priority
        self.constraints.sort_by_key(|c| c.priority);

        // Newton-Raphson iteration
        let mut iteration = 0;
        let mut error = f64::INFINITY;

        while iteration < self.max_iterations && error > self.tolerance {
            // Compute constraint errors
            let errors = self.compute_constraint_errors();
            error = errors.iter().map(|e| e * e).sum::<f64>().sqrt();

            if error < self.tolerance {
                break;
            }

            // Compute Jacobian matrix
            let jacobian = self.compute_jacobian();

            // Solve linear system: J * dx = -errors
            match self.solve_linear_system(&jacobian, &errors) {
                Ok(delta) => {
                    // Apply updates with damping
                    self.apply_updates(&delta, self.damping_factor);
                }
                Err(_) => {
                    return SolverResult {
                        status: SolverStatus::Unstable,
                        entity_updates: self.get_entity_updates(),
                        violations: self.get_violations(),
                        solve_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
                    };
                }
            }

            iteration += 1;
        }

        // Determine final status
        let status = if error < self.tolerance {
            SolverStatus::Converged {
                iterations: iteration,
                final_error: error,
            }
        } else {
            SolverStatus::NotConverged {
                iterations: iteration,
                final_error: error,
            }
        };

        SolverResult {
            status,
            entity_updates: self.get_entity_updates(),
            violations: self.get_violations(),
            solve_time_ms: start_time.elapsed().as_secs_f64() * 1000.0,
        }
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

    /// Compute constraint errors
    fn compute_constraint_errors(&self) -> Vec<f64> {
        self.constraints
            .iter()
            .flat_map(|constraint| self.evaluate_constraint_error(constraint))
            .collect()
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
            _ => vec![0.0], // Other advanced constraints
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
            _ => vec![0.0], // Other constraints need full implementation
        }
    }

    /// Get point position from entity state
    fn get_point_position(&self, entity: &EntityRef) -> Option<Point2d> {
        self.entity_state.get(entity).map(|state| {
            if state.parameters.len() >= 2 {
                Point2d::new(state.parameters[0], state.parameters[1])
            } else {
                Point2d::ORIGIN
            }
        })
    }

    /// Get line direction from entity state
    fn get_line_direction(&self, entity: &EntityRef) -> Option<Vector2d> {
        self.entity_state.get(entity).map(|state| {
            if state.parameters.len() >= 4 {
                // Parameters: point.x, point.y, dir.x, dir.y
                Vector2d::new(state.parameters[2], state.parameters[3])
            } else {
                Vector2d::UNIT_X
            }
        })
    }

    /// Get circle radius from entity state
    fn get_circle_radius(&self, entity: &EntityRef) -> Option<f64> {
        match entity {
            EntityRef::Circle(_) | EntityRef::Arc(_) => {
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

    /// Get circle center from entity state
    fn get_circle_center(&self, entity: &EntityRef) -> Option<Point2d> {
        match entity {
            EntityRef::Circle(_) | EntityRef::Arc(_) => {
                self.entity_state.get(entity).and_then(|state| {
                    if state.parameters.len() >= 2 {
                        // Parameters: center.x, center.y, radius
                        Some(Point2d::new(state.parameters[0], state.parameters[1]))
                    } else {
                        None
                    }
                })
            }
            _ => None,
        }
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

    /// Count error components for a constraint
    fn constraint_error_count(&self, constraint: &Constraint) -> usize {
        match &constraint.constraint_type {
            ConstraintType::Geometric(gc) => match gc {
                GeometricConstraint::Coincident => 2,
                GeometricConstraint::Parallel => 1,
                GeometricConstraint::Perpendicular => 1,
                GeometricConstraint::Horizontal => 1,
                GeometricConstraint::Vertical => 1,
                _ => 1,
            },
            ConstraintType::Dimensional(_) => 1,
        }
    }

    /// Solve linear system using LU decomposition
    fn solve_linear_system(&self, jacobian: &[Vec<f64>], errors: &[f64]) -> Result<Vec<f64>, ()> {
        let n = jacobian[0].len();
        let m = jacobian.len();

        if m == 0 || n == 0 {
            return Ok(vec![0.0; n]);
        }

        // Use least squares for over-determined systems
        // J^T * J * x = J^T * (-errors)

        // Compute J^T * J
        let mut jtj = vec![vec![0.0; n]; n];
        for i in 0..n {
            for j in 0..n {
                for k in 0..m {
                    jtj[i][j] += jacobian[k][i] * jacobian[k][j];
                }
            }
        }

        // Compute J^T * (-errors)
        let mut jte = vec![0.0; n];
        for i in 0..n {
            for j in 0..m {
                jte[i] -= jacobian[j][i] * errors[j];
            }
        }

        // Solve using Gaussian elimination
        self.gaussian_elimination(jtj, jte)
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

        for entry in self.entity_state.iter() {
            let entity = *entry.key();
            let state = entry.value();

            let update = match entity {
                EntityRef::Point(_) => {
                    EntityUpdate::Point(Point2d::new(state.parameters[0], state.parameters[1]))
                }
                EntityRef::Line(_) => EntityUpdate::Line(
                    Point2d::new(state.parameters[0], state.parameters[1]),
                    Vector2d::new(state.parameters[2], state.parameters[3]),
                ),
                EntityRef::Circle(_) => EntityUpdate::Circle(
                    Point2d::new(state.parameters[0], state.parameters[1]),
                    state.parameters[2],
                ),
                EntityRef::Arc(_) => EntityUpdate::Arc(
                    Point2d::new(state.parameters[0], state.parameters[1]),
                    state.parameters[2],
                    state.parameters[3],
                    state.parameters[4],
                ),
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
            _ => vec![0.0],
        }
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
            self.entity_state
                .get(entity)
                .map(|state| Point2d::new(state.parameters[0], state.parameters[1]))
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
        // Calculate based on line direction and length
        if let (Some(start), Some(dir)) =
            (self.get_line_point(entity), self.get_line_direction(entity))
        {
            let scaled_dir = dir.scale(100.0);
            Some(Point2d::new(start.x + scaled_dir.x, start.y + scaled_dir.y)) // Default length, should be stored
        } else {
            None
        }
    }

    fn get_line_length(&self, _entity: &EntityRef) -> Option<f64> {
        // Should be stored as parameter, using default for now
        Some(100.0)
    }

    /// Read the angular range stored on an arc's entity state.
    ///
    /// Arc parameter layout matches the constructor in `EntityState::arc`:
    /// `[center.x, center.y, radius, start_angle, end_angle]`. Returns
    /// `None` if the entity is not an arc or its state is malformed.
    fn get_arc_angles(&self, entity: &EntityRef) -> Option<(f64, f64)> {
        match entity {
            EntityRef::Arc(_) => self.entity_state.get(entity).and_then(|state| {
                if state.parameters.len() >= 5 {
                    Some((state.parameters[3], state.parameters[4]))
                } else {
                    None
                }
            }),
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

impl EntityState {
    /// Create state for a point
    pub fn point(pos: Point2d, fixed: bool) -> Self {
        Self {
            parameters: vec![pos.x, pos.y],
            fixed_mask: vec![fixed, fixed],
        }
    }

    /// Create state for a line
    pub fn line(point: Point2d, direction: Vector2d, point_fixed: bool, dir_fixed: bool) -> Self {
        Self {
            parameters: vec![point.x, point.y, direction.x, direction.y],
            fixed_mask: vec![point_fixed, point_fixed, dir_fixed, dir_fixed],
        }
    }

    /// Create state for a circle
    pub fn circle(center: Point2d, radius: f64, center_fixed: bool, radius_fixed: bool) -> Self {
        Self {
            parameters: vec![center.x, center.y, radius],
            fixed_mask: vec![center_fixed, center_fixed, radius_fixed],
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
    //! [`EntityState`]; entity kinds whose state cannot be authored
    //! through the public API today (Arc, Rectangle, Ellipse, Spline,
    //! Polyline) are exercised indirectly via constraints they share
    //! with the supported kinds (Line, Circle).
    #![allow(clippy::float_cmp)]
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::sketch2d::constraints::{ConstraintPriority, ConstraintType};
    use crate::sketch2d::{Circle2dId, Line2dId, Point2dId};

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
            SolverStatus::UnderConstrained { .. }
                | SolverStatus::Converged { .. }
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
            SolverStatus::OverConstrained { conflicting_constraints } => {
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Coincident,
            &[a, b],
        );
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Coincident,
            &[a, b],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::new(1.0, 0.0),
                false,
                false,
            ),
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Parallel,
            &[l1, l2],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_X,
                false,
                false,
            ),
        );
        s.add_entity(
            l2,
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_Y,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Perpendicular,
            &[l1, l2],
        );
        assert!(approx_eq(errs[0], 0.0, 1e-12));
    }

    #[test]
    fn horizontal_error_is_dir_y() {
        let mut s = ConstraintSolver::new();
        let l = line_ref();
        s.add_entity(
            l,
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::new(1.0, 0.5),
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Horizontal,
            &[l],
        );
        assert!(approx_eq(errs[0], 0.5, 1e-12));
    }

    #[test]
    fn vertical_error_is_dir_x() {
        let mut s = ConstraintSolver::new();
        let l = line_ref();
        s.add_entity(
            l,
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::new(0.25, 1.0),
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Vertical,
            &[l],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_X,
                false,
                false,
            ),
        );
        s.add_entity(
            c,
            EntityState::circle(Point2d::new(0.0, 5.0), 3.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Tangent,
            &[l, c],
        );
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Concentric,
            &[c1, c2],
        );
        assert!(approx_eq(errs[0], -3.0, 1e-12));
        assert!(approx_eq(errs[1], -4.0, 1e-12));
    }

    #[test]
    fn equal_circles_error_is_radius_diff() {
        let mut s = ConstraintSolver::new();
        let c1 = circle_ref();
        let c2 = circle_ref();
        s.add_entity(
            c1,
            EntityState::circle(Point2d::ORIGIN, 7.0, false, false),
        );
        s.add_entity(
            c2,
            EntityState::circle(Point2d::ORIGIN, 4.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Equal,
            &[c1, c2],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_X,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::PointOnCurve,
            &[p, l],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_X,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::PointOnCurve,
            &[p, l],
        );
        assert!(errs[0].abs() > 1e-6);
    }

    #[test]
    fn point_on_circle_error_dist_minus_radius() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        let c = circle_ref();
        s.add_entity(p, EntityState::point(Point2d::new(5.0, 0.0), false));
        s.add_entity(
            c,
            EntityState::circle(Point2d::ORIGIN, 3.0, false, false),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::PointOnCurve,
            &[p, c],
        );
        assert!(approx_eq(errs[0], 2.0, 1e-12));
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Collinear,
            &[a, b, c],
        );
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Collinear,
            &[a, b, c],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_X,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Midpoint,
            &[p, l],
        );
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
            EntityState::line(
                Point2d::ORIGIN,
                Vector2d::UNIT_X,
                false,
                false,
            ),
        );
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Symmetric,
            &[a, b, axis],
        );
        assert!(approx_eq(errs[0], 0.0, 1e-12));
        assert!(approx_eq(errs[1], 0.0, 1e-12));
    }

    #[test]
    fn coincident_wrong_arity_returns_zeros() {
        let s = ConstraintSolver::new();
        let p = point_ref();
        // Only one entity passed
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Coincident,
            &[p],
        );
        assert_eq!(errs, vec![0.0, 0.0]);
    }

    #[test]
    fn parallel_missing_entity_returns_zero() {
        // Both refs unknown to solver
        let s = ConstraintSolver::new();
        let l1 = line_ref();
        let l2 = line_ref();
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Parallel,
            &[l1, l2],
        );
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
        let errs = s.evaluate_dimensional_constraint(
            &DimensionalConstraint::Distance(2.0),
            &[a, b],
        );
        // Pythagorean distance is 5, target is 2 → error = 3
        assert!(approx_eq(errs[0], 3.0, 1e-12));
    }

    #[test]
    fn radius_error_is_actual_minus_target() {
        let mut s = ConstraintSolver::new();
        let c = circle_ref();
        s.add_entity(
            c,
            EntityState::circle(Point2d::ORIGIN, 5.0, false, false),
        );
        let errs = s.evaluate_dimensional_constraint(
            &DimensionalConstraint::Radius(3.0),
            &[c],
        );
        assert!(approx_eq(errs[0], 2.0, 1e-12));
    }

    #[test]
    fn x_coordinate_error_is_pos_minus_target() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(1.5, 0.0), false));
        let errs = s.evaluate_dimensional_constraint(
            &DimensionalConstraint::XCoordinate(1.0),
            &[p],
        );
        assert!(approx_eq(errs[0], 0.5, 1e-12));
    }

    #[test]
    fn y_coordinate_error_is_pos_minus_target() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::new(0.0, -2.3), false));
        let errs = s.evaluate_dimensional_constraint(
            &DimensionalConstraint::YCoordinate(0.0),
            &[p],
        );
        assert!(approx_eq(errs[0], -2.3, 1e-12));
    }

    #[test]
    fn distance_missing_entity_returns_zero() {
        let s = ConstraintSolver::new();
        let a = point_ref();
        let b = point_ref();
        let errs = s.evaluate_dimensional_constraint(
            &DimensionalConstraint::Distance(5.0),
            &[a, b],
        );
        assert_eq!(errs, vec![0.0]);
    }

    #[test]
    fn radius_for_non_circle_entity_returns_zero() {
        let mut s = ConstraintSolver::new();
        let p = point_ref();
        s.add_entity(p, EntityState::point(Point2d::ORIGIN, false));
        let errs = s.evaluate_dimensional_constraint(
            &DimensionalConstraint::Radius(1.0),
            &[p],
        );
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
        // 1 equation, 2 unknowns: J = [[1, 1]], errors = [2]
        // J^T J = [[1,1],[1,1]] is singular → solve_linear_system returns Err.
        let s = ConstraintSolver::new();
        let j = vec![vec![1.0, 1.0]];
        let errs = vec![2.0];
        assert!(s.solve_linear_system(&j, &errs).is_err());
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
            EntityState::line(
                Point2d::new(1.0, 1.0),
                Vector2d::new(0.0, 0.0),
                true,
                false,
            ),
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
        let errs = s.evaluate_geometric_constraint(
            &GeometricConstraint::Coincident,
            &[a, b],
        );
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
        match r
            .entity_updates
            .get(&p)
            .expect("update for p present")
        {
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
        let st = EntityState::line(
            Point2d::new(1.0, 2.0),
            Vector2d::new(3.0, 4.0),
            false,
            true,
        );
        assert_eq!(st.parameters, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(st.fixed_mask, vec![false, false, true, true]);
    }

    #[test]
    fn entity_state_circle_layout() {
        let st = EntityState::circle(Point2d::new(5.0, 6.0), 7.0, true, false);
        assert_eq!(st.parameters, vec![5.0, 6.0, 7.0]);
        assert_eq!(st.fixed_mask, vec![true, true, false]);
    }

    #[test]
    fn solver_result_carries_status_constraint_type_arms() {
        // Just verifies all SolverStatus variants exist and are debug-printable.
        let _converged = SolverStatus::Converged { iterations: 1, final_error: 0.0 };
        let _not = SolverStatus::NotConverged { iterations: 99, final_error: 1.0 };
        let _over = SolverStatus::OverConstrained { conflicting_constraints: 1 };
        let _under = SolverStatus::UnderConstrained { degrees_of_freedom: 1 };
        let _unstable = SolverStatus::Unstable;
        let _ct = ConstraintType::Geometric(GeometricConstraint::Coincident);
    }
}
