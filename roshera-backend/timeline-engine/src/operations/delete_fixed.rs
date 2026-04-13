//! Delete operation implementation

use async_trait::async_trait;
use crate::{
    TimelineError, TimelineResult,
    Operation, OperationOutputs, SideEffect,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    entity_mapping::get_entity_mapping,
};

/// Implementation of delete operation
pub struct DeleteOp;

#[async_trait]
impl OperationImpl for DeleteOp {
    fn operation_type(&self) -> &'static str {
        "delete"
    }
    
    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()> {
        if let Operation::Delete { entities } = operation {
            // Validate all entities exist
            for entity_id in entities {
                let entity = context.get_entity(*entity_id)?;
                
                // Check if already deleted
                if entity.is_deleted {
                    return Err(TimelineError::ValidationError(
                        format!("Entity {:?} is already deleted", entity_id)
                    ));
                }
                
                // Check for dependencies
                let dependents = context.find_dependent_entities(*entity_id);
                if !dependents.is_empty() {
                    // For now, we'll allow deletion but record it as a side effect
                    // In a production system, this might need a cascade option
                }
            }
            
            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Delete operation".to_string()
            ))
        }
    }
    
    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs> {
        if let Operation::Delete { entities } = operation {
            let mut deleted_ids = Vec::new();
            let mut side_effects = Vec::new();
            let mapping = get_entity_mapping();
            
            // Process each entity to delete
            for entity_id in entities {
                // Get the entity
                let _entity = context.get_entity(*entity_id)?;
                
                // Check for dependencies
                let dependents = context.find_dependent_entities(*entity_id);
                
                // If there are dependents, cascade delete them
                let mut cascaded_entities = Vec::new();
                if !dependents.is_empty() {
                    for dep_id in &dependents {
                        // Mark dependent as deleted
                        context.mark_entity_deleted(*dep_id)?;
                        mapping.remove(*dep_id);
                        cascaded_entities.push(*dep_id);
                        deleted_ids.push(*dep_id);
                    }
                    
                    // Record cascade as a side effect
                    side_effects.push(SideEffect {
                        effect_type: "cascade_delete".to_string(),
                        description: format!("Cascaded deletion of {} dependent entities", dependents.len()),
                        entities: cascaded_entities,
                    });
                }
                
                // Mark the main entity as deleted
                context.mark_entity_deleted(*entity_id)?;
                mapping.remove(*entity_id);
                deleted_ids.push(*entity_id);
                
                // Update operation counts
                context.increment_entities_deleted(1);
                context.increment_geometry_ops();
            }
            
            // Create output
            let outputs = OperationOutputs {
                created: vec![],
                modified: vec![],
                deleted: deleted_ids,
                side_effects,
            };
            
            Ok(outputs)
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Delete operation".to_string()
            ))
        }
    }
    
    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Delete { entities } = operation {
            ResourceEstimate {
                memory_bytes: 100 * entities.len() as u64,
                time_ms: 1 * entities.len() as u64,
                entities_created: 0,
                entities_modified: 0,
            }
        } else {
            ResourceEstimate {
                memory_bytes: 0,
                time_ms: 0,
                entities_created: 0,
                entities_modified: 0,
            }
        }
    }
}