//! Modification operations

use crate::{
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    EntityType, Modification, Operation, OperationOutputs, TimelineError, TimelineResult,
};
use async_trait::async_trait;

/// Implementation for modification operations
pub struct ModifyOp;

#[async_trait]
impl OperationImpl for ModifyOp {
    fn operation_type(&self) -> &'static str {
        "modify"
    }

    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Modify {
            entity,
            modifications,
        } = operation
        {
            // Verify entity exists
            let entity_state = context.get_entity(*entity)?;

            // Validate entity is not deleted
            if entity_state.is_deleted {
                return Err(TimelineError::EntityNotFound(*entity));
            }

            // Validate modifications
            if modifications.is_empty() {
                return Err(TimelineError::ValidationError(
                    "No modifications specified".to_string(),
                ));
            }

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Modify operation".to_string(),
            ))
        }
    }

    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Modify {
            entity,
            modifications,
        } = operation
        {
            // Get the entity
            let mut entity_state = context.get_entity(*entity)?;

            // Apply each modification
            for modification in modifications {
                match modification {
                    Modification::SetName(name) => {
                        if let Some(obj) = entity_state.properties.as_object_mut() {
                            obj.insert("name".to_string(), serde_json::json!(name));
                        }
                    }
                    Modification::SetColor(color) => {
                        if let Some(obj) = entity_state.properties.as_object_mut() {
                            obj.insert("color".to_string(), serde_json::json!(color));
                        }
                    }
                    Modification::SetMaterial(material) => {
                        if let Some(obj) = entity_state.properties.as_object_mut() {
                            obj.insert("material".to_string(), serde_json::json!(material));
                        }
                    }
                    Modification::SetVisible(visible) => {
                        if let Some(obj) = entity_state.properties.as_object_mut() {
                            obj.insert("visible".to_string(), serde_json::json!(visible));
                        }
                    }
                    Modification::SetProperty(key, value) => {
                        if let Some(obj) = entity_state.properties.as_object_mut() {
                            obj.insert(key.clone(), value.clone());
                        }
                    }
                }
            }

            // Mark as modified
            if let Some(obj) = entity_state.properties.as_object_mut() {
                obj.insert(
                    "last_modified".to_string(),
                    serde_json::json!(chrono::Utc::now().to_rfc3339()),
                );
            }

            // Update entity in context
            context.update_entity(*entity, entity_state)?;
            context.increment_geometry_ops();

            // Create output
            let outputs = OperationOutputs {
                created: vec![],
                modified: vec![*entity],
                deleted: vec![],
                side_effects: vec![],
            };

            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Modify operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, _operation: &Operation) -> ResourceEstimate {
        ResourceEstimate {
            memory_bytes: 1000,
            time_ms: 5,
            entities_created: 0,
            entities_modified: 1,
        }
    }
}
