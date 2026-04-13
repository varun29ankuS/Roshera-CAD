//! Core trait system for parametric CAD primitives
//!
//! This module defines the fundamental traits that all primitives must implement
//! to meet world-class CAD requirements for parametric modeling, exact geometry,
//! and complete topological representation.

use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    edge::EdgeId, face::FaceId, solid::SolidId, topology_builder::BRepModel, vertex::VertexId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::error::Error;
use std::fmt;

/// Core trait that all CAD primitives must implement
///
/// This trait ensures every primitive supports:
/// - Parametric construction and updates
/// - Complete B-Rep topology generation
/// - Exact analytical geometry
/// - Manifold validation
/// - History tracking
pub trait Primitive: Send + Sync + std::fmt::Debug {
    /// Parameter type specific to this primitive
    type Parameters: Clone + Serialize + for<'de> Deserialize<'de> + std::fmt::Debug;

    /// Create primitive from parameters with full B-Rep topology
    ///
    /// # Arguments
    /// * `params` - Validated parameters for primitive construction
    /// * `model` - B-Rep model to add topology to
    ///
    /// # Returns
    /// * `Ok(SolidId)` - ID of created solid with complete topology
    /// * `Err(PrimitiveError)` - Invalid parameters or construction failure
    ///
    /// # Requirements
    /// - Must generate complete B-Rep: vertices, edges, faces, shells
    /// - Must use exact analytical geometry (no approximations)
    /// - Must produce manifold solid unless explicitly non-manifold
    /// - Must validate Euler characteristic: V - E + F = 2
    fn create(params: Self::Parameters, model: &mut BRepModel) -> Result<SolidId, PrimitiveError>;

    /// Update existing primitive with new parameters
    ///
    /// # Arguments
    /// * `solid_id` - ID of existing primitive solid
    /// * `params` - New parameters to apply
    /// * `model` - B-Rep model containing the primitive
    ///
    /// # Returns
    /// * `Ok(())` - Parameters updated, topology rebuilt
    /// * `Err(PrimitiveError)` - Invalid parameters or update failure
    ///
    /// # Requirements
    /// - Must preserve stable IDs where possible
    /// - Must maintain topological consistency
    /// - Must be efficient for small parameter changes
    fn update_parameters(
        solid_id: SolidId,
        params: Self::Parameters,
        model: &mut BRepModel,
    ) -> Result<(), PrimitiveError>;

    /// Get current parameters of existing primitive
    ///
    /// # Arguments
    /// * `solid_id` - ID of existing primitive solid
    /// * `model` - B-Rep model containing the primitive
    ///
    /// # Returns
    /// * `Ok(Parameters)` - Current parameter values
    /// * `Err(PrimitiveError)` - Primitive not found or invalid
    fn get_parameters(
        solid_id: SolidId,
        model: &BRepModel,
    ) -> Result<Self::Parameters, PrimitiveError>;

    /// Validate primitive topology and geometry
    ///
    /// # Arguments
    /// * `solid_id` - ID of primitive to validate
    /// * `model` - B-Rep model containing the primitive
    ///
    /// # Returns
    /// * `Ok(ValidationReport)` - Validation results with any issues
    /// * `Err(PrimitiveError)` - Validation failed to run
    ///
    /// # Checks
    /// - Euler characteristic: V - E + F = 2 for simple solids
    /// - Manifold properties (closed, orientable)
    /// - Edge-face adjacency consistency
    /// - Surface/curve analytical properties
    fn validate(solid_id: SolidId, model: &BRepModel) -> Result<ValidationReport, PrimitiveError>;

    /// Get primitive type name for identification
    fn primitive_type() -> &'static str;

    /// Get parameter schema for UI/validation
    fn parameter_schema() -> ParameterSchema;
}

/// Parametric surface trait for exact analytical representation
///
/// All CAD surfaces must support exact evaluation - no mesh approximations
pub trait ParametricSurface: Send + Sync {
    /// Evaluate surface point at (u,v) parameters
    ///
    /// # Arguments
    /// * `u` - First parameter coordinate
    /// * `v` - Second parameter coordinate
    ///
    /// # Returns
    /// * Exact 3D point on surface
    ///
    /// # Requirements
    /// - Must use analytical formulas (no interpolation)
    /// - Must handle parameter boundaries correctly
    /// - Must be consistent with derivative calculations
    fn evaluate(&self, u: f64, v: f64) -> Point3;

    /// Calculate surface normal at (u,v) parameters
    ///
    /// # Arguments
    /// * `u` - First parameter coordinate  
    /// * `v` - Second parameter coordinate
    ///
    /// # Returns
    /// * Unit normal vector pointing outward from solid
    ///
    /// # Requirements
    /// - Must be analytically computed (not finite difference)
    /// - Must be unit length and outward-pointing
    /// - Must handle degenerate cases (poles, seams)
    fn normal(&self, u: f64, v: f64) -> Vector3;

    /// Get parameter domain bounds
    ///
    /// # Returns
    /// * (u_min, u_max, v_min, v_max) - Parameter space bounds
    fn parameter_bounds(&self) -> (f64, f64, f64, f64);

    /// Check if surface is closed in u or v direction
    ///
    /// # Returns
    /// * (u_closed, v_closed) - Closure flags for each parameter
    fn is_closed(&self) -> (bool, bool);

    /// Check if surface is periodic in u or v direction
    ///
    /// # Returns
    /// * (u_period, v_period) - Period values (None if not periodic)
    fn periodicity(&self) -> (Option<f64>, Option<f64>);
}

/// Parametric curve trait for exact analytical representation
///
/// All CAD curves must support exact evaluation - no polyline approximations  
pub trait ParametricCurve: Send + Sync {
    /// Evaluate curve point at t parameter
    ///
    /// # Arguments
    /// * `t` - Parameter coordinate
    ///
    /// # Returns
    /// * Exact 3D point on curve
    ///
    /// # Requirements
    /// - Must use analytical formulas (no interpolation)
    /// - Must handle parameter boundaries correctly
    fn evaluate(&self, t: f64) -> Point3;

    /// Calculate curve tangent at t parameter
    ///
    /// # Arguments
    /// * `t` - Parameter coordinate
    ///
    /// # Returns
    /// * Tangent vector (not necessarily unit length)
    ///
    /// # Requirements
    /// - Must be analytically computed
    /// - Direction must be consistent with parameter increase
    fn derivative(&self, t: f64) -> Vector3;

    /// Get parameter domain bounds
    ///
    /// # Returns
    /// * (t_min, t_max) - Parameter space bounds
    fn parameter_bounds(&self) -> (f64, f64);

    /// Check if curve is closed (forms a loop)
    ///
    /// # Returns
    /// * true if evaluate(t_min) == evaluate(t_max)
    fn is_closed(&self) -> bool;

    /// Check if curve is periodic
    ///
    /// # Returns
    /// * Some(period) if periodic, None otherwise
    fn periodicity(&self) -> Option<f64>;
}

/// Primitive construction errors
#[derive(Debug, Clone)]
pub enum PrimitiveError {
    /// Invalid parameter values
    InvalidParameters {
        parameter: String,
        value: String,
        constraint: String,
    },
    /// Topology construction failed
    TopologyError {
        message: String,
        euler_characteristic: Option<i32>,
    },
    /// Primitive not found in model
    NotFound { solid_id: SolidId },
    /// Non-manifold geometry detected
    NonManifold { issues: Vec<String> },
    /// Numerical precision issues
    NumericalInstability {
        operation: String,
        precision_loss: f64,
    },
    /// Geometry operation error
    GeometryError { operation: String, details: String },
    /// Invalid input provided
    InvalidInput {
        input: String,
        expected: String,
        received: String,
    },
    /// Invalid parameter (single parameter variant for backward compatibility)
    InvalidParameter {
        name: String,
        value: String,
        reason: String,
    },
    /// Invalid topology configuration
    InvalidTopology {
        entity: String,
        issue: String,
        suggestion: String,
    },
    /// Invalid geometry configuration
    InvalidGeometry { entity: String, reason: String },
    /// Math operation error
    MathError { operation: String, details: String },
}

impl fmt::Display for PrimitiveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PrimitiveError::InvalidParameters {
                parameter,
                value,
                constraint,
            } => {
                write!(
                    f,
                    "Invalid parameter '{}' with value '{}': {}",
                    parameter, value, constraint
                )
            }
            PrimitiveError::TopologyError { message, .. } => {
                write!(f, "Topology error: {}", message)
            }
            PrimitiveError::NotFound { solid_id } => {
                write!(f, "Primitive not found with solid ID: {}", solid_id)
            }
            PrimitiveError::NonManifold { issues } => {
                write!(f, "Non-manifold geometry detected: {}", issues.join(", "))
            }
            PrimitiveError::NumericalInstability {
                operation,
                precision_loss,
            } => {
                write!(
                    f,
                    "Numerical instability in operation '{}': precision loss {}",
                    operation, precision_loss
                )
            }
            PrimitiveError::GeometryError { operation, details } => {
                write!(f, "Geometry operation '{}' failed: {}", operation, details)
            }
            PrimitiveError::InvalidInput {
                input,
                expected,
                received,
            } => {
                write!(
                    f,
                    "Invalid input '{}': expected {}, received {}",
                    input, expected, received
                )
            }
            PrimitiveError::InvalidParameter {
                name,
                value,
                reason,
            } => {
                write!(f, "Invalid parameter '{}' = '{}': {}", name, value, reason)
            }
            PrimitiveError::InvalidTopology {
                entity,
                issue,
                suggestion,
            } => {
                write!(
                    f,
                    "Invalid topology in {}: {}. Suggestion: {}",
                    entity, issue, suggestion
                )
            }
            PrimitiveError::InvalidGeometry { entity, reason } => {
                write!(f, "Invalid geometry for {}: {}", entity, reason)
            }
            PrimitiveError::MathError { operation, details } => {
                write!(f, "Math error in '{}': {}", operation, details)
            }
        }
    }
}

impl Error for PrimitiveError {}

impl From<crate::math::MathError> for PrimitiveError {
    fn from(err: crate::math::MathError) -> Self {
        PrimitiveError::MathError {
            operation: "math_operation".to_string(),
            details: format!("{}", err),
        }
    }
}

/// Topology validation report
#[derive(Debug, Clone)]
pub struct ValidationReport {
    /// Overall validation success
    pub is_valid: bool,
    /// Euler characteristic (should be 2 for simple solids)
    pub euler_characteristic: i32,
    /// Manifold validation results
    pub manifold_check: ManifoldStatus,
    /// Individual issues found
    pub issues: Vec<ValidationIssue>,
    /// Performance metrics
    pub metrics: ValidationMetrics,
}

/// Manifold topology status
#[derive(Debug, Clone)]
pub enum ManifoldStatus {
    /// Proper manifold solid
    Manifold,
    /// Non-manifold but valid for advanced modeling
    NonManifoldValid,
    /// Invalid topology (cracks, open edges)
    Invalid { open_edges: Vec<EdgeId> },
}

/// Individual validation issue
#[derive(Debug, Clone)]
pub struct ValidationIssue {
    /// Issue severity level
    pub severity: IssueSeverity,
    /// Human-readable description
    pub description: String,
    /// Related topology entities
    pub entities: Vec<EntityRef>,
    /// Suggested fix (if available)
    pub suggested_fix: Option<String>,
}

/// Issue severity levels
#[derive(Debug, Clone, PartialEq)]
pub enum IssueSeverity {
    /// Critical error - primitive unusable
    Error,
    /// Non-critical warning
    Warning,
    /// Informational note
    Info,
}

/// Reference to any topology entity
#[derive(Debug, Clone)]
pub enum EntityRef {
    Vertex(VertexId),
    Edge(EdgeId),
    Face(FaceId),
    Solid(SolidId),
}

/// Validation performance metrics
#[derive(Debug, Clone)]
pub struct ValidationMetrics {
    /// Time taken for validation
    pub duration_ms: f64,
    /// Number of entities checked
    pub entities_checked: usize,
    /// Memory usage during validation
    pub memory_used_kb: usize,
}

/// Parameter schema for UI generation and validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterSchema {
    /// Schema version for compatibility
    pub version: String,
    /// Parameter definitions
    pub parameters: Vec<ParameterDefinition>,
    /// Parameter constraints and relationships
    pub constraints: Vec<ParameterConstraint>,
}

/// Individual parameter definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDefinition {
    /// Parameter name
    pub name: String,
    /// Human-readable display name
    pub display_name: String,
    /// Parameter description
    pub description: String,
    /// Data type
    pub param_type: ParameterType,
    /// Default value
    pub default_value: serde_json::Value,
    /// Value constraints
    pub constraints: Vec<ValueConstraint>,
    /// Units (if applicable)
    pub units: Option<String>,
}

/// Parameter data types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterType {
    /// Floating-point number
    Float { precision: Option<u32> },
    /// Integer number
    Integer,
    /// Boolean flag
    Boolean,
    /// Text string
    String { max_length: Option<usize> },
    /// 3D point/vector
    Point3D,
    /// Color value
    Color,
    /// Material reference
    Material,
    /// Enumerated choice
    Enum { choices: Vec<String> },
}

/// Value constraints for parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ValueConstraint {
    /// Minimum value (inclusive)
    MinValue(f64),
    /// Maximum value (inclusive)
    MaxValue(f64),
    /// Must be positive (> 0)
    Positive,
    /// Must be non-negative (>= 0)
    NonNegative,
    /// Must be one of specified values
    OneOf(Vec<serde_json::Value>),
    /// Custom validation rule
    Custom { rule: String, message: String },
}

/// Relationships between parameters
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterConstraint {
    /// One parameter must be less than another
    LessThan { param1: String, param2: String },
    /// Parameters must sum to specific value
    Sum { params: Vec<String>, target: f64 },
    /// Conditional constraint (if param1 == value, then constraint applies)
    Conditional {
        condition: String,
        condition_value: serde_json::Value,
        constraint: Box<ParameterConstraint>,
    },
    /// Custom relationship rule
    Custom { rule: String, message: String },
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_parameter_schema_serialization() {
//         let schema = ParameterSchema {
//             version: "1.0".to_string(),
//             parameters: vec![
//                 ParameterDefinition {
//                     name: "width".to_string(),
//                     display_name: "Width".to_string(),
//                     description: "Box width dimension".to_string(),
//                     param_type: ParameterType::Float { precision: Some(3) },
//                     default_value: serde_json::json!(10.0),
//                     constraints: vec![ValueConstraint::Positive],
//                     units: Some("mm".to_string()),
//                 }
//             ],
//             constraints: vec![],
//         };
//
//         // Should serialize/deserialize without issues
//         let json = serde_json::to_string(&schema).unwrap();
//         let _deserialized: ParameterSchema = serde_json::from_str(&json).unwrap();
//     }
//
//     #[test]
//     fn test_primitive_error_display() {
//         let error = PrimitiveError::InvalidParameters {
//             parameter: "radius".to_string(),
//             value: "-5.0".to_string(),
//             constraint: "must be positive".to_string(),
//         };
//
//         let display = format!("{}", error);
//         assert!(display.contains("radius"));
//         assert!(display.contains("-5.0"));
//         assert!(display.contains("positive"));
//     }
// }
