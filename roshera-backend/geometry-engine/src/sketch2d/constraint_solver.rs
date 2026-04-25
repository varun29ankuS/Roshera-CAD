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
        let mut param_index = 0;

        for entry in self.entity_state.iter() {
            let entity = entry.key();
            let state = entry.value();

            for (i, &fixed) in state.fixed_mask.iter().enumerate() {
                if !fixed {
                    // Perturb parameter
                    let original = state.parameters[i];

                    // Forward difference
                    self.perturb_parameter(entity, i, original + h);
                    let errors_plus = self.compute_constraint_errors();

                    self.perturb_parameter(entity, i, original - h);
                    let errors_minus = self.compute_constraint_errors();

                    // Restore original
                    self.perturb_parameter(entity, i, original);

                    // Compute derivatives
                    for (j, (ep, em)) in errors_plus.iter().zip(errors_minus.iter()).enumerate() {
                        jacobian[j][param_index] = (ep - em) / (2.0 * h);
                    }

                    param_index += 1;
                }
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
                EntityRef::Ellipse(_) => {
                    // For now, return point update as placeholder
                    EntityUpdate::Point(Point2d::new(state.parameters[0], state.parameters[1]))
                }
                EntityRef::Spline(_) => {
                    // For now, return point update as placeholder
                    EntityUpdate::Point(Point2d::new(state.parameters[0], state.parameters[1]))
                }
                EntityRef::Polyline(_) => {
                    // For now, return point update as placeholder
                    EntityUpdate::Point(Point2d::new(state.parameters[0], state.parameters[1]))
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

    /// Evaluate G2 continuity (curvature continuity)
    fn evaluate_g2_continuity(&self, curve1: &EntityRef, curve2: &EntityRef) -> Vec<f64> {
        // Get curvature at connection point
        if let (Some(k1), Some(k2)) = (
            self.get_curve_curvature_at_end(curve1),
            self.get_curve_curvature_at_start(curve2),
        ) {
            // Curvatures should match
            vec![k1 - k2, 0.0, 0.0] // Placeholder for higher order terms
        } else {
            vec![0.0, 0.0, 0.0]
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
        // For now, use line point as start
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

    fn get_curve_tangent_at_end(&self, entity: &EntityRef) -> Option<Vector2d> {
        // Simplified - should compute actual tangent
        self.get_line_direction(entity)
    }

    fn get_curve_tangent_at_start(&self, entity: &EntityRef) -> Option<Vector2d> {
        // Simplified - should compute actual tangent
        self.get_line_direction(entity)
    }

    fn get_curve_curvature_at_end(&self, entity: &EntityRef) -> Option<f64> {
        // Simplified - should compute actual curvature
        match entity {
            EntityRef::Circle(_) | EntityRef::Arc(_) => {
                self.get_circle_radius(entity).map(|r| 1.0 / r)
            }
            EntityRef::Line(_) => Some(0.0),
            _ => None,
        }
    }

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
