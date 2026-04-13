//! Registry for operation implementations

use super::{OperationImpl, ResourceEstimate};
use crate::Operation;
use dashmap::DashMap;
use std::sync::Arc;

/// Registry of operation implementations
pub struct OperationRegistry {
    /// Registered implementations by operation type
    implementations: DashMap<&'static str, Arc<dyn OperationImpl>>,
}

impl OperationRegistry {
    /// Create a new registry
    pub fn new() -> Self {
        Self {
            implementations: DashMap::new(),
        }
    }

    /// Register an operation implementation
    pub fn register<T: OperationImpl + 'static>(&self, implementation: T) {
        let operation_type = implementation.operation_type();
        self.implementations
            .insert(operation_type, Arc::new(implementation));

        tracing::info!("Registered operation: {}", operation_type);
    }

    /// Get implementation for an operation
    pub fn get_implementation(&self, operation: &Operation) -> Option<Arc<dyn OperationImpl>> {
        let operation_type = get_operation_type(operation);
        self.implementations
            .get(operation_type)
            .map(|entry| entry.clone())
    }

    /// Get resource estimate for an operation
    pub fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Some(impl_) = self.get_implementation(operation) {
            impl_.estimate_resources(operation)
        } else {
            ResourceEstimate::default()
        }
    }

    /// Get number of registered operations
    pub fn count(&self) -> usize {
        self.implementations.len()
    }

    /// Get list of registered operation types
    pub fn registered_types(&self) -> Vec<&'static str> {
        self.implementations
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }
}

impl Default for OperationRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Get operation type string from operation enum
fn get_operation_type(operation: &Operation) -> &'static str {
    match operation {
        Operation::CreateSketch { .. } => "create_sketch",
        Operation::CreatePrimitive { .. } => "create_primitive",
        Operation::Extrude { .. } => "extrude",
        Operation::Revolve { .. } => "revolve",
        Operation::Loft { .. } => "loft",
        Operation::Sweep { .. } => "sweep",
        Operation::BooleanUnion { .. } => "boolean_union",
        Operation::BooleanIntersection { .. } => "boolean_intersection",
        Operation::BooleanDifference { .. } => "boolean_difference",
        Operation::Fillet { .. } => "fillet",
        Operation::Chamfer { .. } => "chamfer",
        Operation::Pattern { .. } => "pattern",
        Operation::Transform { .. } => "transform",
        Operation::Delete { .. } => "delete",
        Operation::Modify { .. } => "modify",
        Operation::CreateCheckpoint { .. } => "create_checkpoint",
        Operation::Batch { .. } => "batch",
        Operation::Boolean { .. } => "boolean",
        Operation::Generic { .. } => "generic",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{OperationOutputs, TimelineResult};
    use async_trait::async_trait;

    struct TestOperation;

    #[async_trait]
    impl OperationImpl for TestOperation {
        fn operation_type(&self) -> &'static str {
            "test_operation"
        }

        async fn validate(
            &self,
            _operation: &Operation,
            _context: &super::super::ExecutionContext,
        ) -> TimelineResult<()> {
            Ok(())
        }

        async fn execute(
            &self,
            _operation: &Operation,
            _context: &mut super::super::ExecutionContext,
        ) -> TimelineResult<OperationOutputs> {
            Ok(OperationOutputs::default())
        }
    }

    #[test]
    fn test_registry() {
        let registry = OperationRegistry::new();

        // Register test operation
        registry.register(TestOperation);

        assert_eq!(registry.count(), 1);
        assert!(registry.registered_types().contains(&"test_operation"));
    }
}
