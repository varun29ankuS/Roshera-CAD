//! Conflict resolution for branch merging

use super::merge::{
    ConflictResolution, ConflictStrategy, ConflictType, MergeConflict, MergeResult,
};
use crate::{EntityId, EventId, Operation, TimelineError, TimelineEvent, TimelineResult};
use std::collections::{HashMap, HashSet};

/// Conflict resolver for branch merging
pub struct ConflictResolver {
    /// Tolerance for numerical comparisons
    tolerance: f64,

    /// AI resolution service (if available)
    ai_service: Option<AIResolutionService>,
}

/// AI resolution service interface
pub struct AIResolutionService {
    /// API endpoint
    endpoint: String,

    /// Model to use
    model: String,

    /// Timeout in seconds
    timeout_secs: u64,
}

impl ConflictResolver {
    /// Create a new conflict resolver
    pub fn new() -> Self {
        Self {
            tolerance: 1e-10,
            ai_service: None,
        }
    }

    /// Set AI resolution service
    pub fn with_ai_service(mut self, service: AIResolutionService) -> Self {
        self.ai_service = Some(service);
        self
    }

    /// Resolve a set of conflicts
    pub async fn resolve_conflicts(
        &self,
        conflicts: &mut Vec<MergeConflict>,
        strategy: &ConflictStrategy,
        context: &ResolutionContext,
    ) -> TimelineResult<ResolutionReport> {
        let mut report = ResolutionReport::default();

        for conflict in conflicts.iter_mut() {
            match self
                .resolve_single_conflict(conflict, strategy, context)
                .await
            {
                Ok(resolution) => {
                    conflict.resolution = Some(resolution.clone());
                    report.resolved.push(conflict.clone());

                    match resolution {
                        ConflictResolution::UseSource => report.source_wins += 1,
                        ConflictResolution::UseTarget => report.target_wins += 1,
                        ConflictResolution::MergeBoth { .. } => report.merged += 1,
                        ConflictResolution::Skip => report.skipped += 1,
                        ConflictResolution::Custom { .. } => report.custom += 1,
                    }
                }
                Err(e) => {
                    report.failed.push((conflict.clone(), e));
                }
            }
        }

        Ok(report)
    }

    /// Resolve a single conflict
    async fn resolve_single_conflict(
        &self,
        conflict: &MergeConflict,
        strategy: &ConflictStrategy,
        context: &ResolutionContext,
    ) -> TimelineResult<ConflictResolution> {
        match strategy {
            ConflictStrategy::PreferSource => Ok(ConflictResolution::UseSource),

            ConflictStrategy::PreferTarget => Ok(ConflictResolution::UseTarget),

            ConflictStrategy::PreferNewest => {
                let use_source = conflict
                    .source_event
                    .as_ref()
                    .map(|e| e.timestamp)
                    .unwrap_or_default()
                    > conflict
                        .target_event
                        .as_ref()
                        .map(|e| e.timestamp)
                        .unwrap_or_default();

                if use_source {
                    Ok(ConflictResolution::UseSource)
                } else {
                    Ok(ConflictResolution::UseTarget)
                }
            }

            ConflictStrategy::Manual => {
                // For manual resolution, we need user input
                // In this implementation, we'll skip for now
                Ok(ConflictResolution::Skip)
            }

            ConflictStrategy::AI { model, criteria } => {
                self.resolve_with_ai(conflict, model, criteria, context)
                    .await
            }
        }
    }

    /// Resolve conflict using AI
    async fn resolve_with_ai(
        &self,
        conflict: &MergeConflict,
        model: &str,
        criteria: &[String],
        context: &ResolutionContext,
    ) -> TimelineResult<ConflictResolution> {
        if let Some(ai_service) = &self.ai_service {
            // Prepare prompt for AI
            let prompt = self.build_ai_prompt(conflict, criteria, context);

            // In a real implementation, this would call the AI service
            // For now, we'll use a heuristic based on the conflict type
            match conflict.conflict_type {
                ConflictType::ConcurrentModification => {
                    // Try to merge both modifications
                    if let (Some(source), Some(target)) =
                        (&conflict.source_event, &conflict.target_event)
                    {
                        if let Some(merged_op) =
                            self.try_merge_operations(&source.operation, &target.operation)
                        {
                            return Ok(ConflictResolution::MergeBoth { merged_op });
                        }
                    }

                    // If merge fails, prefer the one with better optimization criteria
                    Ok(self.select_by_criteria(conflict, criteria))
                }

                ConflictType::DeleteModify => {
                    // Usually prefer keeping the entity (modification over deletion)
                    if conflict
                        .source_event
                        .as_ref()
                        .map(|e| matches!(e.operation, Operation::Delete { .. }))
                        .unwrap_or(false)
                    {
                        Ok(ConflictResolution::UseTarget)
                    } else {
                        Ok(ConflictResolution::UseSource)
                    }
                }

                ConflictType::OperationConflict => {
                    // Use criteria to decide
                    Ok(self.select_by_criteria(conflict, criteria))
                }

                ConflictType::DependencyConflict => {
                    // Skip to avoid breaking dependencies
                    Ok(ConflictResolution::Skip)
                }

                ConflictType::TopologicalConflict => {
                    // Prefer the operation that maintains valid topology
                    Ok(self.resolve_topological_conflict(conflict, context))
                }
            }
        } else {
            // No AI service, fall back to simple heuristics
            // Prefer newest based on timestamps
            let use_source = conflict
                .source_event
                .as_ref()
                .map(|e| e.timestamp)
                .unwrap_or_default()
                > conflict
                    .target_event
                    .as_ref()
                    .map(|e| e.timestamp)
                    .unwrap_or_default();

            if use_source {
                Ok(ConflictResolution::UseSource)
            } else {
                Ok(ConflictResolution::UseTarget)
            }
        }
    }

    /// Build prompt for AI resolution
    fn build_ai_prompt(
        &self,
        conflict: &MergeConflict,
        criteria: &[String],
        context: &ResolutionContext,
    ) -> String {
        let mut prompt = format!(
            "Resolve merge conflict for entity {}:\n\n",
            conflict.entity_id
        );

        prompt.push_str(&format!("Conflict Type: {:?}\n", conflict.conflict_type));

        if let Some(source) = &conflict.source_event {
            prompt.push_str(&format!("\nSource Operation: {:?}\n", source.operation));
            prompt.push_str(&format!("Source Timestamp: {}\n", source.timestamp));
        }

        if let Some(target) = &conflict.target_event {
            prompt.push_str(&format!("\nTarget Operation: {:?}\n", target.operation));
            prompt.push_str(&format!("Target Timestamp: {}\n", target.timestamp));
        }

        prompt.push_str("\nOptimization Criteria:\n");
        for criterion in criteria {
            prompt.push_str(&format!("- {}\n", criterion));
        }

        prompt.push_str("\nContext:\n");
        prompt.push_str(&format!(
            "- Affected entities: {}\n",
            context.affected_entities.len()
        ));
        prompt.push_str(&format!("- Branch purpose: {}\n", context.branch_purpose));

        prompt
    }

    /// Try to merge two operations
    fn try_merge_operations(&self, op1: &Operation, op2: &Operation) -> Option<Operation> {
        match (op1, op2) {
            // Transform operations can be combined
            (
                Operation::Transform {
                    entities: e1,
                    transformation: t1,
                },
                Operation::Transform {
                    entities: e2,
                    transformation: t2,
                },
            ) => {
                // If same entities, combine transformations
                if e1 == e2 {
                    let combined_transform = self.multiply_matrices(t1, t2);
                    Some(Operation::Transform {
                        entities: e1.clone(),
                        transformation: combined_transform,
                    })
                } else {
                    // Different entities - create batch operation
                    Some(Operation::Batch {
                        operations: vec![op1.clone(), op2.clone()],
                        description: "Merged concurrent transforms".to_string(),
                    })
                }
            }

            // Modify operations might be mergeable
            (
                Operation::Modify {
                    entity: e1,
                    modifications: m1,
                },
                Operation::Modify {
                    entity: e2,
                    modifications: m2,
                },
            ) if e1 == e2 => {
                // Merge modifications (concatenate both lists)
                let mut merged_mods = m1.clone();
                merged_mods.extend(m2.clone());

                Some(Operation::Modify {
                    entity: *e1,
                    modifications: merged_mods,
                })
            }

            // Pattern operations on same features
            (
                Operation::Pattern {
                    features: f1,
                    pattern_type: _pt1,
                },
                Operation::Pattern {
                    features: f2,
                    pattern_type: _pt2,
                },
            ) if f1 == f2 => {
                // Combine patterns into a batch
                Some(Operation::Batch {
                    operations: vec![op1.clone(), op2.clone()],
                    description: "Merged concurrent patterns".to_string(),
                })
            }

            // Default: can't merge
            _ => None,
        }
    }

    /// Multiply two 4x4 transformation matrices
    fn multiply_matrices(&self, m1: &[[f64; 4]; 4], m2: &[[f64; 4]; 4]) -> [[f64; 4]; 4] {
        let mut result = [[0.0; 4]; 4];

        for i in 0..4 {
            for j in 0..4 {
                for k in 0..4 {
                    result[i][j] += m1[i][k] * m2[k][j];
                }
            }
        }

        result
    }

    /// Select resolution based on optimization criteria
    fn select_by_criteria(
        &self,
        conflict: &MergeConflict,
        criteria: &[String],
    ) -> ConflictResolution {
        // Simple heuristic based on common criteria
        for criterion in criteria {
            match criterion.as_str() {
                "minimize_weight" | "reduce_mass" => {
                    // Prefer operations that remove material
                    if let Some(source) = &conflict.source_event {
                        if matches!(
                            source.operation,
                            Operation::BooleanDifference { .. } | Operation::Delete { .. }
                        ) {
                            return ConflictResolution::UseSource;
                        }
                    }
                    if let Some(target) = &conflict.target_event {
                        if matches!(
                            target.operation,
                            Operation::BooleanDifference { .. } | Operation::Delete { .. }
                        ) {
                            return ConflictResolution::UseTarget;
                        }
                    }
                }

                "maximize_strength" => {
                    // Prefer operations that add material or reinforcement
                    if let Some(source) = &conflict.source_event {
                        if matches!(
                            source.operation,
                            Operation::BooleanUnion { .. } | Operation::Fillet { .. }
                        ) {
                            return ConflictResolution::UseSource;
                        }
                    }
                    if let Some(target) = &conflict.target_event {
                        if matches!(
                            target.operation,
                            Operation::BooleanUnion { .. } | Operation::Fillet { .. }
                        ) {
                            return ConflictResolution::UseTarget;
                        }
                    }
                }

                "improve_manufacturability" => {
                    // Prefer operations that simplify geometry
                    if let Some(source) = &conflict.source_event {
                        if matches!(
                            source.operation,
                            Operation::Fillet { .. } | Operation::Chamfer { .. }
                        ) {
                            return ConflictResolution::UseSource;
                        }
                    }
                }

                _ => {}
            }
        }

        // Default: prefer newest based on timestamps
        if let (Some(source), Some(target)) = (&conflict.source_event, &conflict.target_event) {
            if source.timestamp > target.timestamp {
                ConflictResolution::UseSource
            } else {
                ConflictResolution::UseTarget
            }
        } else {
            ConflictResolution::Skip
        }
    }

    /// Resolve topological conflict
    fn resolve_topological_conflict(
        &self,
        conflict: &MergeConflict,
        context: &ResolutionContext,
    ) -> ConflictResolution {
        // For topological conflicts, we need to ensure the result maintains valid topology
        // This is a simplified implementation

        // Check if source operation maintains topology validity
        if let Some(source) = &conflict.source_event {
            if self.preserves_topology(&source.operation, context) {
                return ConflictResolution::UseSource;
            }
        }

        // Check if target operation maintains topology validity
        if let Some(target) = &conflict.target_event {
            if self.preserves_topology(&target.operation, context) {
                return ConflictResolution::UseTarget;
            }
        }

        // Neither preserves topology - skip
        ConflictResolution::Skip
    }

    /// Check if operation preserves valid topology
    fn preserves_topology(&self, operation: &Operation, context: &ResolutionContext) -> bool {
        // Simplified check - in reality would validate against B-Rep rules
        match operation {
            Operation::Delete { entities } => {
                // Check if deleting any of these entities would create non-manifold geometry
                !entities
                    .iter()
                    .any(|e| context.critical_entities.contains(e))
            }

            Operation::BooleanDifference { target, tools } => {
                // Ensure we're not removing critical material
                !context.critical_entities.contains(target)
                    && tools.iter().all(|t| !context.critical_entities.contains(t))
            }

            _ => true, // Most operations preserve topology
        }
    }
}

/// Context for conflict resolution
pub struct ResolutionContext {
    /// Entities affected by the merge
    pub affected_entities: HashSet<EntityId>,

    /// Critical entities that must be preserved
    pub critical_entities: HashSet<EntityId>,

    /// Branch purpose description
    pub branch_purpose: String,

    /// Current model state
    pub model_state: HashMap<EntityId, EntityState>,
}

/// Entity state for conflict resolution
#[derive(Debug, Clone)]
pub struct EntityState {
    /// Entity type
    pub entity_type: crate::EntityType,

    /// Is entity locked
    pub is_locked: bool,

    /// Dependencies
    pub dependencies: Vec<EntityId>,
}

/// Report of conflict resolution
#[derive(Debug, Default)]
pub struct ResolutionReport {
    /// Successfully resolved conflicts
    pub resolved: Vec<MergeConflict>,

    /// Failed resolutions
    pub failed: Vec<(MergeConflict, TimelineError)>,

    /// Number of conflicts resolved by using source
    pub source_wins: usize,

    /// Number of conflicts resolved by using target
    pub target_wins: usize,

    /// Number of conflicts resolved by merging
    pub merged: usize,

    /// Number of conflicts skipped
    pub skipped: usize,

    /// Number of custom resolutions
    pub custom: usize,
}

impl Default for ConflictResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, BranchId, EventIndex, EventMetadata, OperationInputs, OperationOutputs};
    use chrono::Utc;

    #[tokio::test]
    async fn test_prefer_source_resolution() {
        let resolver = ConflictResolver::new();
        let mut conflict = MergeConflict {
            entity_id: EntityId::new(),
            conflict_type: ConflictType::ConcurrentModification,
            source_event: Some(TimelineEvent {
                id: EventId::new(),
                sequence_number: 1,
                timestamp: Utc::now(),
                author: Author::System,
                operation: Operation::Transform {
                    entities: vec![EntityId::new()],
                    transformation: [[1.0; 4]; 4],
                },
                inputs: OperationInputs {
                    required_entities: vec![],
                    optional_entities: vec![],
                    parameters: serde_json::Value::Null,
                },
                outputs: OperationOutputs::default(),
                metadata: EventMetadata {
                    description: None,
                    branch_id: BranchId::new(),
                    tags: vec![],
                    properties: HashMap::new(),
                },
            }),
            target_event: None,
            resolution: None,
        };

        let context = ResolutionContext {
            affected_entities: HashSet::new(),
            critical_entities: HashSet::new(),
            branch_purpose: "test".to_string(),
            model_state: HashMap::new(),
        };

        let resolution = resolver
            .resolve_single_conflict(&conflict, &ConflictStrategy::PreferSource, &context)
            .await
            .unwrap();

        assert!(matches!(resolution, ConflictResolution::UseSource));
    }

    #[test]
    fn test_transform_merging() {
        let resolver = ConflictResolver::new();
        let entity = EntityId::new();

        let op1 = Operation::Transform {
            entities: vec![entity],
            transformation: [
                [2.0, 0.0, 0.0, 0.0],
                [0.0, 2.0, 0.0, 0.0],
                [0.0, 0.0, 2.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };

        let op2 = Operation::Transform {
            entities: vec![entity],
            transformation: [
                [1.0, 0.0, 0.0, 5.0],
                [0.0, 1.0, 0.0, 0.0],
                [0.0, 0.0, 1.0, 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        };

        let merged = resolver.try_merge_operations(&op1, &op2);
        assert!(merged.is_some());

        if let Some(Operation::Transform { transformation, .. }) = merged {
            // Should scale by 2 then translate by 5 in X
            assert_eq!(transformation[0][3], 5.0);
        }
    }
}
