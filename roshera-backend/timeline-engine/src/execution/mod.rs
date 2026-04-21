//! Execution engine for timeline operations

use crate::{
    BranchId, Operation, OperationInputs, OperationOutputs, TimelineEvent, TimelineResult,
};
use async_trait::async_trait;
use dashmap::DashMap;
use std::sync::Arc;

mod context;
mod executor;
mod registry;
mod validation;

pub use context::{EntityState, EntityStateStore, ExecutionContext};
pub use executor::Executor;
pub use registry::OperationRegistry;
pub use validation::OperationValidator;

/// Trait for implementing operations
#[async_trait]
pub trait OperationImpl: Send + Sync {
    /// Get the operation type this implements
    fn operation_type(&self) -> &'static str;

    /// Validate the operation can be executed
    async fn validate(
        &self,
        operation: &Operation,
        context: &ExecutionContext,
    ) -> TimelineResult<()>;

    /// Execute the operation
    async fn execute(
        &self,
        operation: &Operation,
        context: &mut ExecutionContext,
    ) -> TimelineResult<OperationOutputs>;

    /// Estimate resource usage
    fn estimate_resources(&self, _operation: &Operation) -> ResourceEstimate {
        ResourceEstimate::default()
    }
}

/// Resource usage estimate
#[derive(Debug, Clone, Default)]
pub struct ResourceEstimate {
    /// Estimated memory usage in bytes
    pub memory_bytes: u64,

    /// Estimated computation time in milliseconds
    pub time_ms: u64,

    /// Number of entities that will be created
    pub entities_created: usize,

    /// Number of entities that will be modified
    pub entities_modified: usize,
}

/// Result of operation execution
#[derive(Debug, Clone)]
pub struct ExecutionResult {
    /// The event that was executed
    pub event: TimelineEvent,

    /// Outputs from the operation
    pub outputs: OperationOutputs,

    /// Performance metrics
    pub metrics: ExecutionMetrics,
}

/// Performance metrics for execution
#[derive(Debug, Clone, Default)]
pub struct ExecutionMetrics {
    /// Total execution time in milliseconds
    pub total_time_ms: u64,

    /// Validation time in milliseconds
    pub validation_time_ms: u64,

    /// Execution time in milliseconds
    pub execution_time_ms: u64,

    /// Memory allocated during execution
    pub memory_allocated: u64,

    /// Number of geometry operations performed
    pub geometry_ops: u64,
}

/// Main execution engine
pub struct ExecutionEngine {
    /// Operation implementations
    registry: Arc<OperationRegistry>,

    /// Operation validator
    validator: Arc<OperationValidator>,

    /// Active executors by branch
    executors: Arc<DashMap<BranchId, Arc<Executor>>>,

    /// Configuration
    config: ExecutionConfig,
}

/// Execution configuration
#[derive(Debug, Clone)]
pub struct ExecutionConfig {
    /// Maximum parallel operations
    pub max_parallel_ops: usize,

    /// Operation timeout in seconds
    pub operation_timeout_secs: u64,

    /// Enable validation
    pub enable_validation: bool,

    /// Maximum memory per operation
    pub max_memory_per_op: u64,

    /// Enable performance tracking
    pub enable_metrics: bool,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            max_parallel_ops: 4,
            operation_timeout_secs: 30,
            enable_validation: true,
            max_memory_per_op: 1024 * 1024 * 1024, // 1GB
            enable_metrics: true,
        }
    }
}

impl ExecutionEngine {
    /// Create a new execution engine
    pub fn new(config: ExecutionConfig) -> Self {
        Self {
            registry: Arc::new(OperationRegistry::new()),
            validator: Arc::new(OperationValidator::new()),
            executors: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Register an operation implementation
    pub fn register_operation<T: OperationImpl + 'static>(&self, implementation: T) {
        self.registry.register(implementation);
    }

    /// Execute an operation
    pub async fn execute_operation(
        &self,
        event: &TimelineEvent,
        entity_store: Arc<EntityStateStore>,
    ) -> TimelineResult<ExecutionResult> {
        let start_time = std::time::Instant::now();

        // Get or create executor for branch
        let branch_id = event.metadata.branch_id;
        let executor = self
            .executors
            .entry(branch_id)
            .or_insert_with(|| {
                Arc::new(Executor::new(
                    self.registry.clone(),
                    self.validator.clone(),
                    self.config.clone(),
                ))
            })
            .clone();

        // Execute the operation
        let result = executor.execute(event, entity_store).await?;

        // Record metrics
        let mut metrics = result.metrics.clone();
        metrics.total_time_ms = start_time.elapsed().as_millis() as u64;

        Ok(ExecutionResult {
            event: event.clone(),
            outputs: result.outputs,
            metrics,
        })
    }

    /// Validate an operation without executing
    pub async fn validate_operation(
        &self,
        operation: &Operation,
        inputs: &OperationInputs,
        entity_store: Arc<EntityStateStore>,
    ) -> TimelineResult<()> {
        // Create temporary context
        let context = ExecutionContext::new(BranchId::main(), entity_store);

        // Validate
        self.validator.validate(operation, inputs, &context).await
    }

    /// Get resource estimate for an operation
    pub fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        self.registry.estimate_resources(operation)
    }

    /// Clear executors for a branch
    pub fn clear_branch(&self, branch_id: BranchId) {
        self.executors.remove(&branch_id);
    }

    /// Get execution statistics
    pub fn get_stats(&self) -> ExecutionStats {
        ExecutionStats {
            registered_operations: self.registry.count(),
            active_executors: self.executors.len(),
        }
    }
}

/// Execution statistics
#[derive(Debug, Clone)]
pub struct ExecutionStats {
    /// Number of registered operations
    pub registered_operations: usize,

    /// Number of active executors
    pub active_executors: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_execution_engine_creation() {
        let engine = ExecutionEngine::new(ExecutionConfig::default());
        let stats = engine.get_stats();
        assert_eq!(stats.active_executors, 0);
    }
}
