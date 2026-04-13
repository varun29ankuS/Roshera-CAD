//! Delete operation implementation

use crate::{
    entity_mapping::get_entity_mapping,
    execution::{ExecutionContext, OperationImpl, ResourceEstimate},
    EntityType, Operation, OperationOutputs, SideEffect, TimelineError, TimelineResult,
};
use async_trait::async_trait;
use std::collections::HashSet;

/// Implementation of delete operation
pub struct DeleteOp;

impl DeleteOp {
    /// Find all entities that depend on the given entity
    /// This checks geometric dependencies (e.g., a fillet depends on the edges it's applied to)
    fn find_dependent_entities(
        &self,
        entity_id: &crate::EntityId,
        context: &ExecutionContext,
    ) -> Vec<crate::EntityId> {
        let mut dependents = Vec::new();

        // Get all entities and check their properties for references to this entity
        for entity_type in &[
            EntityType::Sketch,
            EntityType::Solid,
            EntityType::Surface,
            EntityType::Curve,
            EntityType::Point,
        ] {
            let entities = context.get_entities_by_type(*entity_type);
            for entity in entities {
                // Skip deleted entities
                if entity.is_deleted {
                    continue;
                }

                // Check if this entity references the target entity
                if let Some(obj) = entity.properties.as_object() {
                    // Check common dependency fields
                    let dependency_fields = [
                        "parent_id",
                        "sketch_id",
                        "profile",
                        "path",
                        "target",
                        "operand_a",
                        "operand_b",
                        "base_entity",
                        "source_entities",
                    ];

                    for field in &dependency_fields {
                        if let Some(value) = obj.get(*field) {
                            // Check if it's a direct reference
                            if let Some(id_str) = value.as_str() {
                                if let Ok(ref_id) = id_str.parse::<uuid::Uuid>() {
                                    if ref_id == entity_id.0 {
                                        dependents.push(entity.id);
                                        break;
                                    }
                                }
                            }
                            // Check if it's in an array
                            else if let Some(arr) = value.as_array() {
                                for item in arr {
                                    if let Some(id_str) = item.as_str() {
                                        if let Ok(ref_id) = id_str.parse::<uuid::Uuid>() {
                                            if ref_id == entity_id.0 {
                                                dependents.push(entity.id);
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Also check the tools array for boolean operations
                    if let Some(tools) = obj.get("tools") {
                        if let Some(arr) = tools.as_array() {
                            for tool in arr {
                                if let Some(id_str) = tool.as_str() {
                                    if let Ok(ref_id) = id_str.parse::<uuid::Uuid>() {
                                        if ref_id == entity_id.0 {
                                            dependents.push(entity.id);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }

                    // Check edges array for fillet/chamfer operations
                    if let Some(edges) = obj.get("edges") {
                        if let Some(arr) = edges.as_array() {
                            for edge in arr {
                                if let Some(id_str) = edge.as_str() {
                                    if let Ok(ref_id) = id_str.parse::<uuid::Uuid>() {
                                        if ref_id == entity_id.0 {
                                            dependents.push(entity.id);
                                            break;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        // Remove duplicates
        let unique: HashSet<_> = dependents.drain(..).collect();
        dependents.extend(unique);

        dependents
    }
}

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
                    return Err(TimelineError::ValidationError(format!(
                        "Entity {:?} is already deleted",
                        entity_id
                    )));
                }

                // Check for dependencies and warn
                let dependents = self.find_dependent_entities(entity_id, context);
                if !dependents.is_empty() {
                    // In production, we cascade delete dependent entities
                    // This validation just ensures the entity exists
                    tracing::warn!(
                        "Entity {:?} has {} dependent entities that will be cascade deleted",
                        entity_id,
                        dependents.len()
                    );
                }
            }

            Ok(())
        } else {
            Err(TimelineError::InvalidOperation(
                "Expected Delete operation".to_string(),
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

            // First pass: collect all entities to delete including dependents
            let mut all_to_delete = HashSet::new();
            for entity_id in entities {
                all_to_delete.insert(*entity_id);

                // Find and add all dependent entities
                let dependents = self.find_dependent_entities(entity_id, context);
                if !dependents.is_empty() {
                    // Record cascade as a side effect
                    side_effects.push(SideEffect {
                        effect_type: "cascade_delete".to_string(),
                        description: format!(
                            "Cascaded deletion of {} entities dependent on {:?}",
                            dependents.len(),
                            entity_id
                        ),
                        entities: dependents.clone(),
                    });

                    for dep_id in dependents {
                        all_to_delete.insert(dep_id);
                    }
                }
            }

            // Second pass: delete all entities
            for entity_id in all_to_delete {
                // Delete from context
                context.delete_entity(entity_id)?;

                // Remove from entity mapping
                mapping.remove(entity_id);

                // Add to deleted list
                deleted_ids.push(entity_id);

                // Update operation count
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
                "Expected Delete operation".to_string(),
            ))
        }
    }

    fn estimate_resources(&self, operation: &Operation) -> ResourceEstimate {
        if let Operation::Delete { entities } = operation {
            // Estimate includes potential cascade deletes
            // Assume average of 2 dependents per entity as a conservative estimate
            let estimated_total = entities.len() * 3;

            ResourceEstimate {
                memory_bytes: 1000 * estimated_total as u64,
                time_ms: 5 * estimated_total as u64,
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
