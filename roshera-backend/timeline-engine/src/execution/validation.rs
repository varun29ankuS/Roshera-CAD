//! Operation validation

use super::ExecutionContext;
use crate::{
    EntityType, Operation, OperationInputs, TimelineError, TimelineResult,
};

/// Validates operations before execution
pub struct OperationValidator {
    /// Validation rules
    rules: Vec<Box<dyn ValidationRule>>,
}

/// A validation rule
trait ValidationRule: Send + Sync {
    /// Validate an operation
    fn validate(
        &self,
        operation: &Operation,
        inputs: &OperationInputs,
        context: &ExecutionContext,
    ) -> TimelineResult<()>;
}

impl OperationValidator {
    /// Create a new validator
    pub fn new() -> Self {
        let mut validator = Self { rules: Vec::new() };

        // Add default rules
        validator.add_rule(Box::new(EntityExistenceRule));
        validator.add_rule(Box::new(EntityTypeRule));
        validator.add_rule(Box::new(OperationSpecificRule));

        validator
    }

    /// Add a validation rule
    pub fn add_rule(&mut self, rule: Box<dyn ValidationRule>) {
        self.rules.push(rule);
    }

    /// Validate an operation
    pub async fn validate(
        &self,
        operation: &Operation,
        inputs: &OperationInputs,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        // Run all validation rules
        for rule in &self.rules {
            rule.validate(operation, inputs, context)?;
        }

        Ok(())
    }
}

/// Rule: All required entities must exist
struct EntityExistenceRule;

impl ValidationRule for EntityExistenceRule {
    fn validate(
        &self,
        _operation: &Operation,
        inputs: &OperationInputs,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        // Check required entities exist
        for entity_ref in &inputs.required_entities {
            if !context.entity_exists(entity_ref.id) {
                return Err(TimelineError::EntityNotFound(entity_ref.id));
            }
        }

        // Optional entities are... optional
        // But log if they don't exist
        for entity_ref in &inputs.optional_entities {
            if !context.entity_exists(entity_ref.id) {
                tracing::debug!("Optional entity {} not found", entity_ref.id);
            }
        }

        Ok(())
    }
}

/// Rule: Entity types must match expectations
struct EntityTypeRule;

impl ValidationRule for EntityTypeRule {
    fn validate(
        &self,
        _operation: &Operation,
        inputs: &OperationInputs,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        // Check entity types
        for entity_ref in &inputs.required_entities {
            if let Ok(entity) = context.get_entity(entity_ref.id) {
                if entity.entity_type != entity_ref.expected_type {
                    return Err(TimelineError::ValidationError(format!(
                        "Entity {} has type {:?}, expected {:?}",
                        entity_ref.id, entity.entity_type, entity_ref.expected_type
                    )));
                }
            }
        }

        Ok(())
    }
}

/// Rule: Operation-specific validation
struct OperationSpecificRule;

impl ValidationRule for OperationSpecificRule {
    fn validate(
        &self,
        operation: &Operation,
        _inputs: &OperationInputs,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        match operation {
            Operation::Extrude {
                sketch_id,
                distance,
                ..
            } => {
                // Validate extrude parameters
                if *distance <= 0.0 {
                    return Err(TimelineError::ValidationError(
                        "Extrude distance must be positive".to_string(),
                    ));
                }

                // Check sketch exists and is valid
                let sketch = context.get_entity(*sketch_id)?;
                if sketch.entity_type != EntityType::Sketch {
                    return Err(TimelineError::ValidationError(format!(
                        "Entity {} is not a sketch",
                        sketch_id
                    )));
                }
            }

            Operation::Revolve {
                sketch_id, angle, ..
            } => {
                // Validate revolve parameters
                if *angle <= 0.0 || *angle > 360.0 {
                    return Err(TimelineError::ValidationError(
                        "Revolve angle must be between 0 and 360 degrees".to_string(),
                    ));
                }

                // Check sketch exists
                let sketch = context.get_entity(*sketch_id)?;
                if sketch.entity_type != EntityType::Sketch {
                    return Err(TimelineError::ValidationError(format!(
                        "Entity {} is not a sketch",
                        sketch_id
                    )));
                }
            }

            Operation::BooleanUnion { operands } | Operation::BooleanIntersection { operands } => {
                // Need at least 2 operands
                if operands.len() < 2 {
                    return Err(TimelineError::ValidationError(
                        "Boolean operations require at least 2 operands".to_string(),
                    ));
                }

                // All operands must be solids
                for &id in operands {
                    let entity = context.get_entity(id)?;
                    if entity.entity_type != EntityType::Solid {
                        return Err(TimelineError::ValidationError(format!(
                            "Entity {} is not a solid",
                            id
                        )));
                    }
                }
            }

            Operation::BooleanDifference { target, tools } => {
                // Check target is solid
                let target_entity = context.get_entity(*target)?;
                if target_entity.entity_type != EntityType::Solid {
                    return Err(TimelineError::ValidationError(format!(
                        "Target entity {} is not a solid",
                        target
                    )));
                }

                // Need at least one tool
                if tools.is_empty() {
                    return Err(TimelineError::ValidationError(
                        "Boolean difference requires at least one tool".to_string(),
                    ));
                }

                // All tools must be solids
                for &id in tools {
                    let entity = context.get_entity(id)?;
                    if entity.entity_type != EntityType::Solid {
                        return Err(TimelineError::ValidationError(format!(
                            "Tool entity {} is not a solid",
                            id
                        )));
                    }
                }
            }

            Operation::Fillet { edges, radius } => {
                // Validate radius
                if *radius <= 0.0 {
                    return Err(TimelineError::ValidationError(
                        "Fillet radius must be positive".to_string(),
                    ));
                }

                // Need at least one edge
                if edges.is_empty() {
                    return Err(TimelineError::ValidationError(
                        "Fillet requires at least one edge".to_string(),
                    ));
                }

                // All must be edges
                for &id in edges {
                    let entity = context.get_entity(id)?;
                    if entity.entity_type != EntityType::Edge {
                        return Err(TimelineError::ValidationError(format!(
                            "Entity {} is not an edge",
                            id
                        )));
                    }
                }
            }

            Operation::Chamfer {
                edges, distance, ..
            } => {
                // Validate distance
                if *distance <= 0.0 {
                    return Err(TimelineError::ValidationError(
                        "Chamfer distance must be positive".to_string(),
                    ));
                }

                // Need at least one edge
                if edges.is_empty() {
                    return Err(TimelineError::ValidationError(
                        "Chamfer requires at least one edge".to_string(),
                    ));
                }

                // All must be edges
                for &id in edges {
                    let entity = context.get_entity(id)?;
                    if entity.entity_type != EntityType::Edge {
                        return Err(TimelineError::ValidationError(format!(
                            "Entity {} is not an edge",
                            id
                        )));
                    }
                }
            }

            Operation::Transform {
                entities,
                transformation,
            } => {
                // Need at least one entity
                if entities.is_empty() {
                    return Err(TimelineError::ValidationError(
                        "Transform requires at least one entity".to_string(),
                    ));
                }

                // Validate transformation matrix
                // Check it's not degenerate (determinant != 0)
                let det = calculate_determinant_4x4(transformation);
                if det.abs() < 1e-10 {
                    return Err(TimelineError::ValidationError(
                        "Transformation matrix is degenerate".to_string(),
                    ));
                }
            }

            _ => {
                // Other operations have their own specific validations
                // implemented in their respective operation modules
            }
        }

        Ok(())
    }
}

/// Calculate 4x4 matrix determinant
fn calculate_determinant_4x4(m: &[[f64; 4]; 4]) -> f64 {
    // Simplified 4x4 determinant calculation
    let a = m[0][0]
        * (m[1][1] * (m[2][2] * m[3][3] - m[2][3] * m[3][2])
            - m[1][2] * (m[2][1] * m[3][3] - m[2][3] * m[3][1])
            + m[1][3] * (m[2][1] * m[3][2] - m[2][2] * m[3][1]));

    let b = m[0][1]
        * (m[1][0] * (m[2][2] * m[3][3] - m[2][3] * m[3][2])
            - m[1][2] * (m[2][0] * m[3][3] - m[2][3] * m[3][0])
            + m[1][3] * (m[2][0] * m[3][2] - m[2][2] * m[3][0]));

    let c = m[0][2]
        * (m[1][0] * (m[2][1] * m[3][3] - m[2][3] * m[3][1])
            - m[1][1] * (m[2][0] * m[3][3] - m[2][3] * m[3][0])
            + m[1][3] * (m[2][0] * m[3][1] - m[2][1] * m[3][0]));

    let d = m[0][3]
        * (m[1][0] * (m[2][1] * m[3][2] - m[2][2] * m[3][1])
            - m[1][1] * (m[2][0] * m[3][2] - m[2][2] * m[3][0])
            + m[1][2] * (m[2][0] * m[3][1] - m[2][1] * m[3][0]));

    a - b + c - d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{BranchId, EntityId};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_entity_existence_validation() {
        let store = Arc::new(super::super::EntityStateStore::new());
        let context = ExecutionContext::new(BranchId::main(), store);
        let validator = OperationValidator::new();

        let operation = Operation::Transform {
            entities: vec![EntityId::new()], // Non-existent entity
            transformation: [[1.0; 4]; 4],
        };

        let inputs = OperationInputs {
            required_entities: vec![EntityReference {
                id: EntityId::new(),
                expected_type: EntityType::Solid,
                validation: ValidationRequirement::MustExist,
            }],
            optional_entities: vec![],
            parameters: serde_json::Value::Null,
        };

        let result = validator.validate(&operation, &inputs, &context).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_determinant_calculation() {
        // Identity matrix should have determinant 1
        let identity = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];

        let det = calculate_determinant_4x4(&identity);
        assert!((det - 1.0).abs() < 1e-10);
    }
}
