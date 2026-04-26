//! 2D Sketching System for Roshera CAD
//!
//! This module provides a complete 2D sketching foundation for the CAD system,
//! including parametric primitives, geometric constraints, and a constraint solver.
//!
//! # Architecture
//!
//! The 2D sketching system follows these principles:
//! - **Parametric**: All entities are defined by parameters that can be modified
//! - **Constrained**: Geometric and dimensional constraints define relationships
//! - **Solver-based**: A constraint solver maintains consistency
//! - **Production-ready**: No placeholders, full implementations only
//!
//! # Coordinate System
//!
//! Sketches exist in a 2D plane with:
//! - Origin at (0, 0)
//! - X-axis pointing right
//! - Y-axis pointing up
//! - Angles measured counter-clockwise from positive X-axis
//!
//! # Timeline Integration
//!
//! All sketch operations are recorded as timeline events for:
//! - Undo/redo capability
//! - Parametric updates
//! - Design exploration through branching
//!
//! Indexed access into Matrix3 element arrays and entity buffers is the
//! canonical idiom — all `arr[i]` sites use indices bounded by fixed array
//! length or collection length. Matches the numerical-kernel pattern used
//! in nurbs.rs.
#![allow(clippy::indexing_slicing)]

// Core 2D primitives
pub mod arc2d;
pub mod circle2d;
pub mod ellipse2d;
pub mod line2d;
pub mod point2d;
pub mod polyline2d;
pub mod rectangle2d;
pub mod spline2d;

// Constraint system
pub mod constraint_solver;
pub mod constraints;

// Sketch container and management
pub mod sketch;
pub mod sketch_plane;

// Utilities
pub mod pattern;
pub mod sketch_topology;
pub mod sketch_validation;

// Re-export commonly used types
pub use arc2d::{Arc2d, Arc2dId};
pub use circle2d::{Circle2d, Circle2dId};
pub use ellipse2d::{Ellipse2d, Ellipse2dId};
pub use line2d::{Line2d, Line2dId, LineSegment2d, Ray2d};
pub use point2d::{Point2d, Point2dId, Vector2d};
pub use polyline2d::{Polyline2d, Polyline2dId};
pub use rectangle2d::{Rectangle2d, Rectangle2dId};
pub use spline2d::{BSpline2d, NurbsCurve2d, Spline2d, Spline2dId};

pub use constraint_solver::{ConstraintSolver, EntityState, SolverResult, SolverStatus};
pub use constraints::{
    Constraint, ConstraintId, ConstraintPriority, ConstraintStatus, ConstraintType,
    DimensionalConstraint, GeometricConstraint,
};

pub use sketch::{Sketch, SketchId};
pub use sketch_plane::{PlaneOrientation, SketchPlane};

pub use pattern::{PatternId, PatternOperations, PatternParams, PatternResult, PatternType};

// Error types for 2D operations
use thiserror::Error;

#[derive(Debug, Error, Clone, PartialEq)]
pub enum Sketch2dError {
    #[error("Invalid parameter: {parameter} = {value}, must be {constraint}")]
    InvalidParameter {
        parameter: String,
        value: String,
        constraint: String,
    },

    #[error("Degenerate geometry: {entity} - {reason}")]
    DegenerateGeometry { entity: String, reason: String },

    #[error("Constraint conflict: {description}")]
    ConstraintConflict { description: String },

    #[error("Over-constrained: {entity} has {constraints} constraints, maximum is {max}")]
    OverConstrained {
        entity: String,
        constraints: usize,
        max: usize,
    },

    #[error("Under-constrained: {entity} has {constraints} constraints, minimum is {min}")]
    UnderConstrained {
        entity: String,
        constraints: usize,
        min: usize,
    },

    #[error("Solver failed: {reason}")]
    SolverFailed { reason: String },

    #[error("Entity not found: {entity_type} with id {id}")]
    EntityNotFound { entity_type: String, id: String },

    #[error("Invalid topology: {reason}")]
    InvalidTopology { reason: String },

    #[error("Numerical error: {description}")]
    NumericalError { description: String },

    #[error("Invalid operation: {operation} - {reason}")]
    InvalidOperation { operation: String, reason: String },
}

pub type Sketch2dResult<T> = Result<T, Sketch2dError>;

/// Tolerance for 2D operations
#[derive(Debug, Clone, Copy)]
pub struct Tolerance2d {
    /// Distance tolerance in sketch units
    pub distance: f64,
    /// Angular tolerance in radians
    pub angle: f64,
    /// Parameter tolerance for curves
    pub parameter: f64,
}

impl Default for Tolerance2d {
    fn default() -> Self {
        Self {
            distance: 1e-10,
            angle: 1e-12,
            parameter: 1e-12,
        }
    }
}

impl Tolerance2d {
    /// Create tolerance with custom values
    pub fn new(distance: f64, angle: f64, parameter: f64) -> Self {
        Self {
            distance,
            angle,
            parameter,
        }
    }

    /// Check if two distances are equal within tolerance
    pub fn distances_equal(&self, d1: f64, d2: f64) -> bool {
        (d1 - d2).abs() < self.distance
    }

    /// Check if two angles are equal within tolerance
    pub fn angles_equal(&self, a1: f64, a2: f64) -> bool {
        // Handle angle wrapping
        let diff = (a1 - a2).abs();
        let wrapped_diff = (2.0 * std::f64::consts::PI - diff).abs();
        diff.min(wrapped_diff) < self.angle
    }

    /// Check if two parameters are equal within tolerance
    pub fn parameters_equal(&self, p1: f64, p2: f64) -> bool {
        (p1 - p2).abs() < self.parameter
    }
}

/// Common trait for all 2D sketch entities
pub trait SketchEntity2d: Send + Sync {
    /// Get the degrees of freedom for this entity
    fn degrees_of_freedom(&self) -> usize;

    /// Get the current constraint count
    fn constraint_count(&self) -> usize;

    /// Check if entity is fully constrained
    fn is_fully_constrained(&self) -> bool {
        self.constraint_count() >= self.degrees_of_freedom()
    }

    /// Check if entity is over-constrained
    fn is_over_constrained(&self) -> bool {
        self.constraint_count() > self.degrees_of_freedom()
    }

    /// Check if entity is under-constrained
    fn is_under_constrained(&self) -> bool {
        self.constraint_count() < self.degrees_of_freedom()
    }

    /// Get bounding box in 2D space
    fn bounding_box(&self) -> (Point2d, Point2d);

    /// Transform the entity by a 2D transformation matrix
    fn transform(&mut self, matrix: &Matrix3);

    /// Create a deep copy of the entity
    fn clone_entity(&self) -> Box<dyn SketchEntity2d>;
}

/// 2D transformation matrix for sketch operations
#[derive(Debug, Clone, Copy)]
pub struct Matrix3 {
    /// Row-major 3x3 matrix data
    pub data: [[f64; 3]; 3],
}

impl Matrix3 {
    /// Identity matrix
    pub fn identity() -> Self {
        Self {
            data: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    /// Translation matrix
    pub fn translation(x: f64, y: f64) -> Self {
        Self {
            data: [[1.0, 0.0, x], [0.0, 1.0, y], [0.0, 0.0, 1.0]],
        }
    }

    /// Rotation matrix (angle in radians)
    pub fn rotation(angle: f64) -> Self {
        let c = angle.cos();
        let s = angle.sin();
        Self {
            data: [[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    /// Scale matrix
    pub fn scale(sx: f64, sy: f64) -> Self {
        Self {
            data: [[sx, 0.0, 0.0], [0.0, sy, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    /// Transform a 2D point
    pub fn transform_point(&self, point: &Point2d) -> Point2d {
        let x = self.data[0][0] * point.x + self.data[0][1] * point.y + self.data[0][2];
        let y = self.data[1][0] * point.x + self.data[1][1] * point.y + self.data[1][2];
        Point2d::new(x, y)
    }

    /// Matrix multiplication
    pub fn multiply(&self, other: &Matrix3) -> Self {
        let mut result = Self::identity();
        for i in 0..3 {
            for j in 0..3 {
                result.data[i][j] = 0.0;
                for k in 0..3 {
                    result.data[i][j] += self.data[i][k] * other.data[k][j];
                }
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tolerance_distance() {
        let tol = Tolerance2d::default();
        assert!(tol.distances_equal(1.0, 1.0 + 1e-11));
        assert!(!tol.distances_equal(1.0, 1.0 + 1e-9));
    }

    #[test]
    fn test_tolerance_angle() {
        let tol = Tolerance2d::default();
        let pi = std::f64::consts::PI;

        // Test normal case
        assert!(tol.angles_equal(0.5, 0.5 + 1e-13));

        // Test angle wrapping
        assert!(tol.angles_equal(0.0, 2.0 * pi));
        assert!(tol.angles_equal(-pi, pi));
    }

    #[test]
    fn test_matrix_transformation() {
        let p = Point2d::new(1.0, 0.0);

        // Test translation
        let t = Matrix3::translation(2.0, 3.0);
        let p2 = t.transform_point(&p);
        assert_eq!(p2.x, 3.0);
        assert_eq!(p2.y, 3.0);

        // Test rotation (90 degrees)
        let r = Matrix3::rotation(std::f64::consts::PI / 2.0);
        let p3 = r.transform_point(&p);
        assert!((p3.x - 0.0).abs() < 1e-10);
        assert!((p3.y - 1.0).abs() < 1e-10);

        // Test scale
        let s = Matrix3::scale(2.0, 3.0);
        let p4 = s.transform_point(&p);
        assert_eq!(p4.x, 2.0);
        assert_eq!(p4.y, 0.0);
    }
}
