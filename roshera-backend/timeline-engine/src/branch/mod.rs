//! Branch management for timeline-based CAD system
//!
//! This module provides comprehensive branch management functionality
//! for AI-driven design exploration and collaborative workflows.

use crate::error::{TimelineError, TimelineResult};
use crate::types::{
    AIContext, Author, Branch, BranchId, BranchMetadata, BranchPurpose, BranchState, EventIndex,
    ForkPoint, OptimizationObjective,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;

pub mod conflict;
mod merge;
mod strategy;

pub use conflict::ConflictResolver;
pub use merge::{
    ConflictResolution, ConflictStrategy, MergeConflict, MergeResult, MergeStatistics,
    MergeStrategy,
};
pub use strategy::{BranchingStrategy, ExplorationStrategy};

/// Branch manager for the timeline system
pub struct BranchManager {
    /// All branches indexed by ID
    branches: Arc<DashMap<BranchId, Branch>>,

    /// Active branches (not merged or abandoned)
    active_branches: Arc<DashMap<BranchId, ()>>,

    /// Branch relationships (parent -> children)
    branch_tree: Arc<DashMap<BranchId, Vec<BranchId>>>,

    /// AI agent branches for concurrent exploration
    ai_branches: Arc<DashMap<String, Vec<BranchId>>>, // agent_id -> branches

    /// Branch metrics for performance tracking
    metrics: Arc<DashMap<BranchId, BranchMetrics>>,
}

/// Metrics for branch performance and quality
#[derive(Debug, Clone)]
pub struct BranchMetrics {
    /// Number of events in this branch
    pub event_count: usize,

    /// Number of entities created
    pub entities_created: usize,

    /// Number of entities modified
    pub entities_modified: usize,

    /// Total execution time in milliseconds
    pub total_execution_ms: u64,

    /// Quality score (0.0 to 1.0)
    pub quality_score: f64,

    /// Memory usage in bytes
    pub memory_usage: usize,

    /// Last activity timestamp
    pub last_activity: DateTime<Utc>,
}

impl BranchManager {
    /// Create a new branch manager
    pub fn new() -> Self {
        Self {
            branches: Arc::new(DashMap::new()),
            active_branches: Arc::new(DashMap::new()),
            branch_tree: Arc::new(DashMap::new()),
            ai_branches: Arc::new(DashMap::new()),
            metrics: Arc::new(DashMap::new()),
        }
    }

    /// Create a new branch
    pub fn create_branch(
        &self,
        name: String,
        parent: BranchId,
        fork_event: EventIndex,
        author: Author,
        purpose: BranchPurpose,
    ) -> TimelineResult<BranchId> {
        // Validate parent exists
        if !self.branches.contains_key(&parent) {
            return Err(TimelineError::BranchNotFound(parent));
        }

        // Create new branch
        let branch_id = BranchId::new();
        let fork_point = ForkPoint {
            branch_id: parent,
            event_index: fork_event,
            timestamp: Utc::now(),
        };

        let metadata = BranchMetadata {
            created_by: author.clone(),
            created_at: Utc::now(),
            purpose: purpose.clone(),
            ai_context: None,
            checkpoints: Vec::new(),
        };

        let branch = Branch {
            id: branch_id,
            name,
            fork_point,
            parent: Some(parent),
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata,
            protected: false,
            hidden: false,
        };

        // Add to collections
        self.branches.insert(branch_id, branch);
        self.active_branches.insert(branch_id, ());

        // Update parent-child relationships
        self.branch_tree
            .entry(parent)
            .or_insert_with(Vec::new)
            .push(branch_id);

        // Initialize metrics
        self.metrics.insert(
            branch_id,
            BranchMetrics {
                event_count: 0,
                entities_created: 0,
                entities_modified: 0,
                total_execution_ms: 0,
                quality_score: 0.0,
                memory_usage: 0,
                last_activity: Utc::now(),
            },
        );

        // Track AI branches if applicable
        if let BranchPurpose::AIOptimization { .. } = purpose {
            if let Author::AIAgent { id: agent_id, .. } = author {
                self.ai_branches
                    .entry(agent_id)
                    .or_insert_with(Vec::new)
                    .push(branch_id);
            }
        }

        Ok(branch_id)
    }

    /// Create an AI exploration branch
    pub fn create_ai_branch(
        &self,
        parent: BranchId,
        fork_event: EventIndex,
        agent_id: String,
        model: String,
        objective: OptimizationObjective,
        constraints: Vec<crate::DesignConstraint>,
    ) -> TimelineResult<BranchId> {
        let ai_context = AIContext {
            agent_id: agent_id.clone(),
            model,
            objective: format!("{:?}", objective),
            constraints,
            iterations: 0,
            current_score: 0.0,
        };

        let purpose = BranchPurpose::AIOptimization { objective };
        let author = Author::AIAgent {
            id: agent_id.clone(),
            model: "1.0".to_string(),
        };

        let branch_id = self.create_branch(
            format!("AI-{}-{}", agent_id, Utc::now().timestamp()),
            parent,
            fork_event,
            author,
            purpose,
        )?;

        // Set AI context
        if let Some(mut branch) = self.branches.get_mut(&branch_id) {
            branch.metadata.ai_context = Some(ai_context);
        }

        Ok(branch_id)
    }

    /// Get a branch by ID
    pub fn get_branch(&self, id: BranchId) -> Option<Branch> {
        self.branches.get(&id).map(|entry| entry.clone())
    }

    /// Get all active branches
    pub fn get_active_branches(&self) -> Vec<BranchId> {
        self.active_branches
            .iter()
            .map(|entry| *entry.key())
            .collect()
    }

    /// Get child branches of a parent
    pub fn get_child_branches(&self, parent: BranchId) -> Vec<BranchId> {
        self.branch_tree
            .get(&parent)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Get all branches for an AI agent
    pub fn get_ai_branches(&self, agent_id: &str) -> Vec<BranchId> {
        self.ai_branches
            .get(agent_id)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Update branch metrics
    pub fn update_metrics(
        &self,
        branch_id: BranchId,
        event_count: usize,
        entities_created: usize,
        entities_modified: usize,
        execution_ms: u64,
    ) -> TimelineResult<()> {
        let mut metrics = self
            .metrics
            .get_mut(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        metrics.event_count = event_count;
        metrics.entities_created += entities_created;
        metrics.entities_modified += entities_modified;
        metrics.total_execution_ms += execution_ms;
        metrics.last_activity = Utc::now();

        Ok(())
    }

    /// Update quality score for a branch
    pub fn update_quality_score(&self, branch_id: BranchId, score: f64) -> TimelineResult<()> {
        let mut metrics = self
            .metrics
            .get_mut(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        metrics.quality_score = score.clamp(0.0, 1.0);

        // Update AI context if applicable
        if let Some(mut branch) = self.branches.get_mut(&branch_id) {
            if let Some(ref mut ai_context) = branch.metadata.ai_context {
                ai_context.current_score = score;
            }
        }

        Ok(())
    }

    /// Mark a branch as completed
    pub fn complete_branch(&self, branch_id: BranchId, final_score: f64) -> TimelineResult<()> {
        let mut branch = self
            .branches
            .get_mut(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        branch.state = BranchState::Completed { score: final_score };
        self.active_branches.remove(&branch_id);

        Ok(())
    }

    /// Abandon a branch
    pub fn abandon_branch(&self, branch_id: BranchId, reason: String) -> TimelineResult<()> {
        let mut branch = self
            .branches
            .get_mut(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        branch.state = BranchState::Abandoned { reason };
        self.active_branches.remove(&branch_id);

        Ok(())
    }

    /// Find the best branch based on quality scores
    pub fn find_best_branch(&self, parent: BranchId) -> Option<(BranchId, f64)> {
        let children = self.get_child_branches(parent);

        children
            .into_iter()
            .filter_map(|child_id| {
                self.metrics
                    .get(&child_id)
                    .map(|metrics| (child_id, metrics.quality_score))
            })
            .max_by(|(_, score1), (_, score2)| {
                score1
                    .partial_cmp(score2)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }

    /// Get branch statistics
    pub fn get_statistics(&self) -> BranchStatistics {
        let total_branches = self.branches.len();
        let active_branches = self.active_branches.len();
        let ai_agents = self.ai_branches.len();

        let mut total_events = 0;
        let mut total_memory = 0;
        let mut max_depth = 0;

        for entry in self.metrics.iter() {
            let metrics = entry.value();
            total_events += metrics.event_count;
            total_memory += metrics.memory_usage;
        }

        // Calculate max depth
        max_depth = self.calculate_max_depth();

        BranchStatistics {
            total_branches,
            active_branches,
            completed_branches: total_branches - active_branches,
            ai_agents,
            total_events,
            total_memory_mb: (total_memory as f64) / (1024.0 * 1024.0),
            max_depth,
        }
    }

    /// Calculate maximum branch depth
    fn calculate_max_depth(&self) -> usize {
        let mut max_depth = 0;

        for entry in self.branches.iter() {
            let branch = entry.value();
            let depth = self.calculate_branch_depth(branch.id);
            max_depth = max_depth.max(depth);
        }

        max_depth
    }

    /// Calculate depth of a specific branch
    fn calculate_branch_depth(&self, branch_id: BranchId) -> usize {
        let mut depth = 0;
        let mut current = Some(branch_id);

        while let Some(id) = current {
            if let Some(branch) = self.branches.get(&id) {
                current = branch.parent;
                depth += 1;
            } else {
                break;
            }
        }

        depth
    }

    /// Prune inactive branches older than threshold
    pub fn prune_old_branches(&self, days_threshold: i64) -> usize {
        let cutoff_date = Utc::now() - chrono::Duration::days(days_threshold);
        let mut pruned_count = 0;

        let branches_to_prune: Vec<BranchId> = self
            .metrics
            .iter()
            .filter(|entry| {
                let metrics = entry.value();
                let branch_id = *entry.key();

                // Only prune abandoned branches
                if let Some(branch) = self.branches.get(&branch_id) {
                    matches!(branch.state, BranchState::Abandoned { .. })
                        && metrics.last_activity < cutoff_date
                } else {
                    false
                }
            })
            .map(|entry| *entry.key())
            .collect();

        for branch_id in branches_to_prune {
            self.remove_branch(branch_id);
            pruned_count += 1;
        }

        pruned_count
    }

    /// Remove a branch and its data
    fn remove_branch(&self, branch_id: BranchId) {
        // Remove from all collections
        self.branches.remove(&branch_id);
        self.active_branches.remove(&branch_id);
        self.metrics.remove(&branch_id);

        // Remove from parent's children
        for mut entry in self.branch_tree.iter_mut() {
            entry.value_mut().retain(|&id| id != branch_id);
        }

        // Remove from AI branches
        for mut entry in self.ai_branches.iter_mut() {
            entry.value_mut().retain(|&id| id != branch_id);
        }
    }
}

/// Branch statistics
#[derive(Debug, Clone)]
pub struct BranchStatistics {
    /// Total number of branches
    pub total_branches: usize,

    /// Number of active branches
    pub active_branches: usize,

    /// Number of completed branches
    pub completed_branches: usize,

    /// Number of AI agents with branches
    pub ai_agents: usize,

    /// Total events across all branches
    pub total_events: usize,

    /// Total memory usage in MB
    pub total_memory_mb: f64,

    /// Maximum branch depth
    pub max_depth: usize,
}

impl Default for BranchManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_branch_creation() {
        let manager = BranchManager::new();

        // Create main branch
        let main_branch = Branch {
            id: BranchId::main(),
            name: "main".to_string(),
            fork_point: ForkPoint {
                branch_id: BranchId::main(),
                event_index: 0,
                timestamp: Utc::now(),
            },
            parent: None,
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: BranchMetadata {
                created_by: Author::System,
                created_at: Utc::now(),
                purpose: BranchPurpose::UserExploration {
                    description: "Main timeline".to_string(),
                },
                ai_context: None,
                checkpoints: Vec::new(),
            },
            protected: false,
            hidden: false,
        };

        manager.branches.insert(BranchId::main(), main_branch);

        // Create feature branch
        let branch_id = manager
            .create_branch(
                "feature-1".to_string(),
                BranchId::main(),
                10,
                Author::User {
                    id: "user123".to_string(),
                    name: "Test User".to_string(),
                },
                BranchPurpose::Feature {
                    feature_name: "New feature".to_string(),
                },
            )
            .unwrap();

        // Verify branch was created
        assert!(manager.get_branch(branch_id).is_some());
        assert!(manager.active_branches.contains_key(&branch_id));

        // Verify parent-child relationship
        let children = manager.get_child_branches(BranchId::main());
        assert_eq!(children.len(), 1);
        assert_eq!(children[0], branch_id);
    }

    #[test]
    fn test_ai_branch_management() {
        let manager = BranchManager::new();

        // Setup main branch
        let main_branch = Branch {
            id: BranchId::main(),
            name: "main".to_string(),
            fork_point: ForkPoint {
                branch_id: BranchId::main(),
                event_index: 0,
                timestamp: Utc::now(),
            },
            parent: None,
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: BranchMetadata {
                created_by: Author::System,
                created_at: Utc::now(),
                purpose: BranchPurpose::UserExploration {
                    description: "Main timeline".to_string(),
                },
                ai_context: None,
                checkpoints: Vec::new(),
            },
            protected: false,
            hidden: false,
        };

        manager.branches.insert(BranchId::main(), main_branch);

        // Create AI branch
        let ai_branch_id = manager
            .create_ai_branch(
                BranchId::main(),
                5,
                "ai-agent-1".to_string(),
                "gpt-4".to_string(),
                OptimizationObjective::MinimizeWeight,
                vec![],
            )
            .unwrap();

        // Verify AI branch tracking
        let ai_branches = manager.get_ai_branches("ai-agent-1");
        assert_eq!(ai_branches.len(), 1);
        assert_eq!(ai_branches[0], ai_branch_id);

        // Update quality score
        manager.update_quality_score(ai_branch_id, 0.85).unwrap();

        // Complete the branch
        manager.complete_branch(ai_branch_id, 0.85).unwrap();

        // Verify state change
        let branch = manager.get_branch(ai_branch_id).unwrap();
        assert!(matches!(branch.state, BranchState::Completed { score } if score == 0.85));
    }

    #[test]
    fn test_best_branch_selection() {
        let manager = BranchManager::new();

        // Setup main branch
        let main_branch = Branch {
            id: BranchId::main(),
            name: "main".to_string(),
            fork_point: ForkPoint {
                branch_id: BranchId::main(),
                event_index: 0,
                timestamp: Utc::now(),
            },
            parent: None,
            events: Arc::new(DashMap::new()),
            state: BranchState::Active,
            metadata: BranchMetadata {
                created_by: Author::System,
                created_at: Utc::now(),
                purpose: BranchPurpose::UserExploration {
                    description: "Main timeline".to_string(),
                },
                ai_context: None,
                checkpoints: Vec::new(),
            },
            protected: false,
            hidden: false,
        };

        manager.branches.insert(BranchId::main(), main_branch);

        // Create multiple branches with different scores
        let branch1 = manager
            .create_branch(
                "branch-1".to_string(),
                BranchId::main(),
                10,
                Author::System,
                BranchPurpose::AIOptimization {
                    objective: OptimizationObjective::MinimizeWeight,
                },
            )
            .unwrap();

        let branch2 = manager
            .create_branch(
                "branch-2".to_string(),
                BranchId::main(),
                10,
                Author::System,
                BranchPurpose::AIOptimization {
                    objective: OptimizationObjective::MinimizeWeight,
                },
            )
            .unwrap();

        // Set quality scores
        manager.update_quality_score(branch1, 0.7).unwrap();
        manager.update_quality_score(branch2, 0.9).unwrap();

        // Find best branch
        let best = manager.find_best_branch(BranchId::main());
        assert!(best.is_some());

        let (best_id, best_score) = best.unwrap();
        assert_eq!(best_id, branch2);
        assert_eq!(best_score, 0.9);
    }
}
