//! Operation executor

use super::{
    EntityStateStore, ExecutionConfig, ExecutionContext, ExecutionMetrics, ExecutionResult,
    OperationRegistry, OperationValidator,
};
use crate::{OperationOutputs, TimelineError, TimelineEvent, TimelineResult};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

/// Executes operations with proper validation and resource management
pub struct Executor {
    /// Operation registry
    registry: Arc<OperationRegistry>,

    /// Operation validator
    validator: Arc<OperationValidator>,

    /// Configuration
    config: ExecutionConfig,
}

impl Executor {
    /// Create a new executor
    pub fn new(
        registry: Arc<OperationRegistry>,
        validator: Arc<OperationValidator>,
        config: ExecutionConfig,
    ) -> Self {
        Self {
            registry,
            validator,
            config,
        }
    }

    /// Execute a timeline event
    pub async fn execute(
        &self,
        event: &TimelineEvent,
        entity_store: Arc<EntityStateStore>,
    ) -> TimelineResult<ExecutionResult> {
        let mut metrics = ExecutionMetrics::default();
        let start_time = Instant::now();

        // Create execution context
        let mut context = ExecutionContext::new(event.metadata.branch_id, entity_store);

        // Validate if enabled
        if self.config.enable_validation {
            let validation_start = Instant::now();

            self.validator
                .validate(&event.operation, &event.inputs, &context)
                .await?;

            metrics.validation_time_ms = validation_start.elapsed().as_millis() as u64;
        }

        // Get operation implementation
        let implementation = self
            .registry
            .get_implementation(&event.operation)
            .ok_or_else(|| {
                TimelineError::NotImplemented(format!(
                    "Operation not implemented: {:?}",
                    event.operation
                ))
            })?;

        // Execute with timeout
        let execution_start = Instant::now();
        let timeout_duration = Duration::from_secs(self.config.operation_timeout_secs);

        let outputs = match timeout(timeout_duration, async {
            implementation.execute(&event.operation, &mut context).await
        })
        .await
        {
            Ok(Ok(outputs)) => outputs,
            Ok(Err(e)) => return Err(e),
            Err(_) => {
                return Err(TimelineError::ExecutionError(format!(
                    "Operation timed out after {} seconds",
                    self.config.operation_timeout_secs
                )))
            }
        };

        metrics.execution_time_ms = execution_start.elapsed().as_millis() as u64;

        // Validate outputs
        self.validate_outputs(&outputs)?;

        // Update metrics
        metrics.total_time_ms = start_time.elapsed().as_millis() as u64;
        metrics.geometry_ops = context.get_geometry_op_count();
        metrics.memory_allocated = context.get_memory_allocated();

        Ok(ExecutionResult {
            event: event.clone(),
            outputs,
            metrics,
        })
    }

    /// Validate operation outputs
    fn validate_outputs(&self, outputs: &OperationOutputs) -> TimelineResult<()> {
        // Check for duplicate IDs
        let mut seen_ids = std::collections::HashSet::new();

        for entity in &outputs.created {
            if !seen_ids.insert(entity.id) {
                return Err(TimelineError::ValidationError(format!(
                    "Duplicate entity ID in outputs: {}",
                    entity.id
                )));
            }
        }

        // Check modified entities don't overlap with created
        for &id in &outputs.modified {
            if seen_ids.contains(&id) {
                return Err(TimelineError::ValidationError(format!(
                    "Entity {} appears in both created and modified",
                    id
                )));
            }
        }

        // Check deleted entities don't overlap with created or modified
        for &id in &outputs.deleted {
            if seen_ids.contains(&id) {
                return Err(TimelineError::ValidationError(format!(
                    "Entity {} appears in both created/modified and deleted",
                    id
                )));
            }

            if outputs.modified.contains(&id) {
                return Err(TimelineError::ValidationError(format!(
                    "Entity {} appears in both modified and deleted",
                    id
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Author, BranchId, CreatedEntity, EntityType, EventId, EventMetadata, Operation,
        OperationInputs,
    };

    #[tokio::test]
    async fn test_output_validation() {
        let registry = Arc::new(OperationRegistry::new());
        let validator = Arc::new(OperationValidator::new());
        let config = ExecutionConfig::default();

        let executor = Executor::new(registry, validator, config);

        // Test valid outputs
        let outputs = OperationOutputs {
            created: vec![CreatedEntity {
                id: crate::EntityId::new(),
                entity_type: EntityType::Solid,
                name: Some("test".to_string()),
            }],
            modified: vec![],
            deleted: vec![],
            side_effects: vec![],
        };

        assert!(executor.validate_outputs(&outputs).is_ok());

        // Test duplicate in created
        let entity_id = crate::EntityId::new();
        let outputs = OperationOutputs {
            created: vec![
                CreatedEntity {
                    id: entity_id,
                    entity_type: EntityType::Solid,
                    name: Some("test1".to_string()),
                },
                CreatedEntity {
                    id: entity_id, // Duplicate!
                    entity_type: EntityType::Solid,
                    name: Some("test2".to_string()),
                },
            ],
            modified: vec![],
            deleted: vec![],
            side_effects: vec![],
        };

        assert!(executor.validate_outputs(&outputs).is_err());
    }
}
