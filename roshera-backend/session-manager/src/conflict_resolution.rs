//! Conflict resolution using Operational Transformation (OT)
//!
//! This module implements operational transformation algorithms to handle
//! concurrent edits in real-time collaborative CAD sessions.

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use shared_types::AICommand;
use std::sync::Arc;
use uuid::Uuid;

/// Operational Transformation engine for CAD commands
pub struct OTEngine {
    /// Transformation rules for different command pairs
    transform_rules: Arc<DashMap<(String, String), TransformRule>>,
    /// Operation history for each session
    operation_history: Arc<DashMap<String, Vec<Operation>>>,
}

/// Represents an operation that can be transformed
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Operation {
    pub id: Uuid,
    pub command: shared_types::AICommand,
    pub timestamp: u64,
    pub user_id: String,
    pub dependencies: Vec<Uuid>,
}

/// Rule for transforming one operation against another
#[derive(Clone)]
pub struct TransformRule {
    /// Function to transform op1 against op2
    transform_fn: Arc<dyn Fn(&Operation, &Operation) -> Option<Operation> + Send + Sync>,
}

/// Result of transforming operations
#[derive(Debug, Clone)]
pub struct TransformResult {
    pub transformed_op: Operation,
    pub conflicts: Vec<Conflict>,
}

/// Result of applying operations with OT
#[derive(Debug, Clone)]
pub struct TransformedOperation {
    pub original: Operation,
    pub transformed: Operation,
    pub command: shared_types::AICommand,
}

/// Represents a conflict between operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Conflict {
    pub op1: Uuid,
    pub op2: Uuid,
    pub conflict_type: ConflictType,
    pub resolution: ConflictResolution,
}

/// Types of conflicts that can occur
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictType {
    /// Both operations modify the same object
    SameObject,
    /// Operations create conflicting topology
    TopologyConflict,
    /// Operations would result in invalid geometry
    GeometryConflict,
    /// Parent-child relationship conflict
    HierarchyConflict,
}

/// How a conflict was resolved
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConflictResolution {
    /// First operation takes precedence
    FirstWins,
    /// Second operation takes precedence
    SecondWins,
    /// Operations were merged
    Merged,
    /// Both operations were kept with modifications
    BothKept,
    /// Manual resolution required
    ManualRequired,
}

impl OTEngine {
    /// Create a new OT engine with default rules
    pub fn new() -> Self {
        let engine = Self {
            transform_rules: Arc::new(DashMap::new()),
            operation_history: Arc::new(DashMap::new()),
        };

        // Register default transformation rules
        engine.register_default_rules();
        engine
    }

    /// Transform op1 against op2
    pub fn transform(&self, op1: &Operation, op2: &Operation) -> TransformResult {
        let operation_type1 = format!("{:?}", op1.command);
        let operation_type2 = format!("{:?}", op2.command);
        let key = (operation_type1, operation_type2);

        if let Some(rule) = self.transform_rules.get(&key) {
            if let Some(transformed) = (rule.transform_fn)(op1, op2) {
                return TransformResult {
                    transformed_op: transformed,
                    conflicts: vec![],
                };
            }
        }

        // Default: no transformation needed
        TransformResult {
            transformed_op: op1.clone(),
            conflicts: vec![],
        }
    }

    /// Register default transformation rules
    fn register_default_rules(&self) {
        // Transform vs Transform rule
        self.register_rule(
            "transform".to_string(),
            "transform".to_string(),
            Arc::new(|op1, op2| {
                // Both operations transform objects
                match (&op1.command, &op2.command) {
                    (
                        AICommand::Transform {
                            object_id: obj1, ..
                        },
                        AICommand::Transform {
                            object_id: obj2, ..
                        },
                    ) if obj1 == obj2 => {
                        // Same object - need to compose transforms
                        // For now, just return op1 (later wins)
                        Some(op1.clone())
                    }
                    _ => Some(op1.clone()),
                }
            }),
        );

        // Boolean vs Transform rule
        self.register_rule(
            "boolean".to_string(),
            "transform".to_string(),
            Arc::new(|op1, _op2| {
                // Boolean operations take precedence over transforms
                Some(op1.clone())
            }),
        );

        // Create vs Create rule
        self.register_rule(
            "create".to_string(),
            "create".to_string(),
            Arc::new(|op1, _op2| {
                // Both create operations can coexist
                Some(op1.clone())
            }),
        );
    }

    /// Register a transformation rule
    pub fn register_rule(
        &self,
        type1: String,
        type2: String,
        rule: Arc<dyn Fn(&Operation, &Operation) -> Option<Operation> + Send + Sync>,
    ) {
        self.transform_rules
            .insert((type1, type2), TransformRule { transform_fn: rule });
    }

    /// Add operation to history
    pub fn add_operation(&self, session_id: &str, operation: Operation) {
        self.operation_history
            .entry(session_id.to_string())
            .or_insert_with(Vec::new)
            .push(operation);
    }

    /// Get operation history for a session
    pub fn get_history(&self, session_id: &str) -> Vec<Operation> {
        self.operation_history
            .get(session_id)
            .map(|history| history.clone())
            .unwrap_or_default()
    }

    /// Apply operations with conflict resolution
    pub async fn apply_operations(
        &self,
        session_id: &str,
        operations: Vec<Operation>,
    ) -> Result<Vec<TransformedOperation>, String> {
        let mut results = Vec::new();
        let history = self.get_history(session_id);

        for operation in operations {
            let mut transformed_op = operation.clone();

            // Transform against concurrent operations in history
            for concurrent_op in &history {
                if concurrent_op.timestamp > operation.timestamp {
                    let transform_result = self.transform(&transformed_op, concurrent_op);
                    transformed_op = transform_result.transformed_op;
                }
            }

            // Add to history
            self.add_operation(session_id, transformed_op.clone());

            // Create TransformedOperation with the command already available
            results.push(TransformedOperation {
                original: operation,
                transformed: transformed_op.clone(),
                command: transformed_op.command,
            });
        }

        Ok(results)
    }
}

/// CRDT for geometry collaboration
#[derive(Debug, Clone)]
pub struct GeometryCRDT {
    /// Session ID
    session_id: String,
    /// State vector for causality tracking
    state_vector: DashMap<String, u64>,
    /// Pending operations
    pending_ops: DashMap<String, Vec<Operation>>,
}

impl GeometryCRDT {
    /// Create new CRDT for a session
    pub fn new(session_id: String) -> Self {
        Self {
            session_id,
            state_vector: DashMap::new(),
            pending_ops: DashMap::new(),
        }
    }

    /// Apply local operation
    pub fn apply_local(&self, user_id: &str, operation: Operation) {
        // Increment state vector
        let mut counter = self.state_vector.entry(user_id.to_string()).or_insert(0);
        *counter += 1;

        // Add to pending operations
        self.pending_ops
            .entry(user_id.to_string())
            .or_insert_with(Vec::new)
            .push(operation);
    }

    /// Update property of an object (for CRDT state tracking)
    pub fn update_property(
        &self,
        object_id: shared_types::ObjectId,
        _property_name: String,
        _property_value: serde_json::Value,
        timestamp: u64,
    ) {
        // Create a synthetic operation for property updates
        let update_op = Operation {
            id: Uuid::new_v4(),
            command: shared_types::AICommand::Transform {
                object_id,
                transform_type: shared_types::TransformType::Translate {
                    offset: [0.0, 0.0, 0.0], // Placeholder Vector3D
                },
            },
            timestamp,
            user_id: "system".to_string(),
            dependencies: vec![],
        };

        // Apply to local state
        self.apply_local("system", update_op);
    }

    /// Merge with remote CRDT state
    pub fn merge(&self, remote: &GeometryCRDT) -> Vec<Operation> {
        let mut merged_ops = Vec::new();

        // Compare state vectors to find new operations
        for entry in remote.state_vector.iter() {
            let remote_user = entry.key();
            let remote_counter = entry.value();

            let local_counter = self
                .state_vector
                .get(remote_user)
                .map(|c| *c.value())
                .unwrap_or(0);

            if remote_counter > &local_counter {
                // Remote has operations we don't have
                if let Some(ops) = remote.pending_ops.get(remote_user) {
                    for op in ops.iter().skip(local_counter as usize) {
                        merged_ops.push(op.clone());
                    }
                }

                // Update our state vector
                self.state_vector
                    .insert(remote_user.clone(), *remote_counter);
            }
        }

        merged_ops
    }

    /// Get current state vector
    pub fn state_vector(&self) -> Vec<(String, u64)> {
        self.state_vector
            .iter()
            .map(|entry| (entry.key().clone(), *entry.value()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ot_transform() {
        let engine = OTEngine::new();

        let op1 = Operation {
            id: Uuid::new_v4(),
            command: shared_types::AICommand::Transform {
                object_id: shared_types::ObjectId::new_v4(),
                transform_type: shared_types::TransformType::Translate {
                    offset: [10.0, 0.0, 0.0],
                },
            },
            timestamp: 1000,
            user_id: "user1".to_string(),
            dependencies: vec![],
        };

        let op2 = Operation {
            id: Uuid::new_v4(),
            command: shared_types::AICommand::Transform {
                object_id: shared_types::ObjectId::new_v4(),
                transform_type: shared_types::TransformType::Rotate {
                    axis: [0.0, 0.0, 1.0], // Z axis as Vector3D
                    angle_degrees: 45.0,
                },
            },
            timestamp: 1001,
            user_id: "user2".to_string(),
            dependencies: vec![],
        };

        let result = engine.transform(&op1, &op2);
        assert_eq!(result.conflicts.len(), 0);
    }

    #[test]
    fn test_crdt_merge() {
        let crdt1 = GeometryCRDT::new("session1".to_string());
        let crdt2 = GeometryCRDT::new("session1".to_string());

        let op1 = Operation {
            id: Uuid::new_v4(),
            command: shared_types::AICommand::CreatePrimitive {
                shape_type: shared_types::PrimitiveType::Box,
                parameters: shared_types::ShapeParameters::box_params(10.0, 10.0, 10.0),
                position: [0.0, 0.0, 0.0],
                material: None,
            },
            timestamp: 1000,
            user_id: "user1".to_string(),
            dependencies: vec![],
        };

        crdt1.apply_local("user1", op1.clone());

        let merged = crdt2.merge(&crdt1);
        assert_eq!(merged.len(), 1);
    }
}
