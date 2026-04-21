//! Branching strategies for AI-driven design exploration

use super::BranchManager;
use crate::error::{TimelineError, TimelineResult};
use crate::types::{
    Author, BranchId, BranchPurpose, DesignConstraint, EntityId, EventId, EventIndex,
    OptimizationObjective,
};
use std::collections::HashMap;

/// Strategy for creating and managing branches
pub trait BranchingStrategy: Send + Sync {
    /// Determine if a branch should be created
    fn should_branch(&self, context: &BranchingContext) -> bool;

    /// Create branch configurations
    fn create_branches(&self, context: &BranchingContext) -> Vec<BranchConfig>;

    /// Select best branch to merge
    fn select_best_branch(
        &self,
        candidates: &[(BranchId, f64)],
        context: &BranchingContext,
    ) -> Option<BranchId>;

    /// Determine when to prune branches
    fn should_prune_branch(
        &self,
        branch_id: BranchId,
        metrics: &BranchMetrics,
        context: &BranchingContext,
    ) -> bool;
}

/// Context for branching decisions
pub struct BranchingContext {
    /// Current branch
    pub current_branch: BranchId,

    /// Current event index
    pub current_event: EventIndex,

    /// Active optimization objectives
    pub objectives: Vec<OptimizationObjective>,

    /// Design constraints
    pub constraints: Vec<DesignConstraint>,

    /// Number of active branches
    pub active_branch_count: usize,

    /// Available AI agents
    pub available_agents: Vec<String>,

    /// Performance history
    pub performance_history: Vec<f64>,
}

/// Configuration for creating a new branch
pub struct BranchConfig {
    /// Branch name
    pub name: String,

    /// Purpose of the branch
    pub purpose: BranchPurpose,

    /// Author (usually AI agent)
    pub author: Author,

    /// Optimization objective
    pub objective: Option<OptimizationObjective>,

    /// Initial parameters
    pub parameters: HashMap<String, f64>,
}

/// Metrics for branch evaluation
pub struct BranchMetrics {
    /// Quality score (0.0 to 1.0)
    pub quality_score: f64,

    /// Number of events
    pub event_count: usize,

    /// Time since last update
    pub idle_time_ms: u64,

    /// Resource usage
    pub memory_usage: usize,

    /// Convergence rate
    pub convergence_rate: f64,
}

/// Strategy for AI-driven exploration
pub struct ExplorationStrategy {
    /// Maximum number of concurrent branches
    max_branches: usize,

    /// Minimum score improvement to keep branch
    min_improvement: f64,

    /// Maximum idle time before pruning
    max_idle_ms: u64,

    /// Exploration temperature (higher = more exploration)
    temperature: f64,
}

impl ExplorationStrategy {
    /// Create a new exploration strategy
    pub fn new() -> Self {
        Self {
            max_branches: 10,
            min_improvement: 0.05,
            max_idle_ms: 60_000, // 1 minute
            temperature: 0.7,
        }
    }

    /// Create with custom parameters
    pub fn with_params(
        max_branches: usize,
        min_improvement: f64,
        max_idle_ms: u64,
        temperature: f64,
    ) -> Self {
        Self {
            max_branches,
            min_improvement,
            max_idle_ms,
            temperature,
        }
    }

    /// Get initial parameters based on objective
    fn get_initial_parameters(&self, objective: &OptimizationObjective) -> HashMap<String, f64> {
        let mut params = HashMap::new();

        match objective {
            OptimizationObjective::MinimizeWeight => {
                params.insert("wall_thickness_factor".to_string(), 0.8);
                params.insert("material_removal_bias".to_string(), 0.7);
                params.insert("hollowing_threshold".to_string(), 0.5);
            }

            OptimizationObjective::MaximizeStrength => {
                params.insert("wall_thickness_factor".to_string(), 1.2);
                params.insert("reinforcement_bias".to_string(), 0.8);
                params.insert("fillet_radius_factor".to_string(), 1.5);
            }

            OptimizationObjective::MinimizeCost => {
                params.insert("material_efficiency".to_string(), 0.9);
                params.insert("manufacturing_simplicity".to_string(), 0.8);
                params.insert("standard_sizes_bias".to_string(), 0.7);
            }

            OptimizationObjective::MinimizeMaterial => {
                params.insert("material_utilization".to_string(), 0.95);
                params.insert("nesting_efficiency".to_string(), 0.9);
                params.insert("waste_reduction".to_string(), 0.85);
            }

            OptimizationObjective::Custom { .. } => {
                // Custom objectives start with default parameters
                params.insert("exploration_factor".to_string(), 0.5);
            }
        }

        params
    }
}

impl BranchingStrategy for ExplorationStrategy {
    fn should_branch(&self, context: &BranchingContext) -> bool {
        // Don't branch if we're at capacity
        if context.active_branch_count >= self.max_branches {
            return false;
        }

        // Branch if we have multiple objectives
        if context.objectives.len() > 1 {
            return true;
        }

        // Branch if performance is plateauing
        if context.performance_history.len() >= 5 {
            let recent = &context.performance_history[context.performance_history.len() - 5..];
            let variance = calculate_variance(recent);
            if variance < self.min_improvement {
                return true;
            }
        }

        // Probabilistic branching based on temperature
        let branch_probability = self.temperature;
        rand::random::<f64>() < branch_probability
    }

    fn create_branches(&self, context: &BranchingContext) -> Vec<BranchConfig> {
        let mut configs = Vec::new();

        // Create branch for each objective
        for (i, objective) in context.objectives.iter().enumerate() {
            if let Some(agent) = context.available_agents.get(i) {
                configs.push(BranchConfig {
                    name: format!("{:?}-optimization", objective),
                    purpose: BranchPurpose::AIOptimization {
                        objective: objective.clone(),
                    },
                    author: Author::AIAgent {
                        id: agent.clone(),
                        model: "1.0".to_string(),
                    },
                    objective: Some(objective.clone()),
                    parameters: self.get_initial_parameters(objective),
                });
            }
        }

        // Add exploration branch if temperature is high
        if self.temperature > 0.8 && !context.available_agents.is_empty() {
            configs.push(BranchConfig {
                name: "exploration-branch".to_string(),
                purpose: BranchPurpose::UserExploration {
                    description: "AI exploration branch for discovering new design possibilities"
                        .to_string(),
                },
                author: Author::AIAgent {
                    id: context.available_agents[0].clone(),
                    model: "1.0".to_string(),
                },
                objective: None,
                parameters: HashMap::new(),
            });
        }

        configs
    }

    fn select_best_branch(
        &self,
        candidates: &[(BranchId, f64)],
        _context: &BranchingContext,
    ) -> Option<BranchId> {
        if candidates.is_empty() {
            return None;
        }

        // Find branch with highest score. Treat NaN as the smallest value so
        // it cannot win over a finite score.
        let best = candidates
            .iter()
            .max_by(|(_, s1), (_, s2)| s1.partial_cmp(s2).unwrap_or(std::cmp::Ordering::Equal))?;

        // Only select if improvement is significant
        if best.1 > self.min_improvement {
            Some(best.0)
        } else {
            None
        }
    }

    fn should_prune_branch(
        &self,
        _branch_id: BranchId,
        metrics: &BranchMetrics,
        context: &BranchingContext,
    ) -> bool {
        // Prune if idle too long
        if metrics.idle_time_ms > self.max_idle_ms {
            return true;
        }

        // Prune if not improving
        if metrics.convergence_rate < self.min_improvement {
            return true;
        }

        // Prune if quality is too low and we need space
        if context.active_branch_count >= self.max_branches && metrics.quality_score < 0.3 {
            return true;
        }

        false
    }
}

/// Adaptive branching strategy that learns from results
pub struct AdaptiveBranchingStrategy {
    /// Base strategy
    base: ExplorationStrategy,

    /// Success history by objective
    success_rates: HashMap<String, f64>,

    /// Learning rate
    learning_rate: f64,
}

impl AdaptiveBranchingStrategy {
    /// Create new adaptive strategy
    pub fn new() -> Self {
        Self {
            base: ExplorationStrategy::new(),
            success_rates: HashMap::new(),
            learning_rate: 0.1,
        }
    }

    /// Update success rate for an objective
    pub fn update_success_rate(&mut self, objective: &str, success: bool) {
        let current = self.success_rates.get(objective).copied().unwrap_or(0.5);
        let update = if success { 1.0 } else { 0.0 };

        // Exponential moving average
        let new_rate = current * (1.0 - self.learning_rate) + update * self.learning_rate;
        self.success_rates.insert(objective.to_string(), new_rate);
    }
}

impl BranchingStrategy for AdaptiveBranchingStrategy {
    fn should_branch(&self, context: &BranchingContext) -> bool {
        // Use base strategy directly
        self.base.should_branch(context)
    }

    fn create_branches(&self, context: &BranchingContext) -> Vec<BranchConfig> {
        let mut configs = self.base.create_branches(context);

        // Prioritize objectives with higher success rates
        configs.sort_by(|a, b| {
            let score_a = self.get_objective_score(&a.purpose);
            let score_b = self.get_objective_score(&b.purpose);
            score_b
                .partial_cmp(&score_a)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Take top branches based on success rates
        configs.truncate(self.base.max_branches);
        configs
    }

    fn select_best_branch(
        &self,
        candidates: &[(BranchId, f64)],
        context: &BranchingContext,
    ) -> Option<BranchId> {
        self.base.select_best_branch(candidates, context)
    }

    fn should_prune_branch(
        &self,
        branch_id: BranchId,
        metrics: &BranchMetrics,
        context: &BranchingContext,
    ) -> bool {
        self.base.should_prune_branch(branch_id, metrics, context)
    }
}

impl AdaptiveBranchingStrategy {
    /// Get score for an objective based on history
    fn get_objective_score(&self, purpose: &BranchPurpose) -> f64 {
        match purpose {
            BranchPurpose::AIOptimization { objective } => {
                let key = format!("{:?}", objective);
                self.success_rates.get(&key).copied().unwrap_or(0.5)
            }
            _ => 0.5,
        }
    }
}

/// Calculate variance of a slice
fn calculate_variance(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }

    let mean = values.iter().sum::<f64>() / values.len() as f64;
    let variance = values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64;

    variance
}

impl Default for ExplorationStrategy {
    fn default() -> Self {
        Self::new()
    }
}

impl Default for AdaptiveBranchingStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exploration_strategy_branching() {
        let strategy = ExplorationStrategy::new();

        let context = BranchingContext {
            current_branch: BranchId::main(),
            current_event: 10,
            objectives: vec![
                OptimizationObjective::MinimizeWeight,
                OptimizationObjective::MaximizeStrength,
            ],
            constraints: vec![],
            active_branch_count: 5,
            available_agents: vec!["agent1".to_string(), "agent2".to_string()],
            performance_history: vec![0.5, 0.5, 0.5, 0.5, 0.5],
        };

        // Should branch due to multiple objectives
        assert!(strategy.should_branch(&context));

        // Should create branches for each objective
        let branches = strategy.create_branches(&context);
        assert_eq!(branches.len(), 2);
    }

    #[test]
    fn test_adaptive_strategy_learning() {
        let mut strategy = AdaptiveBranchingStrategy::new();

        // Update success rates
        strategy.update_success_rate("MinimizeWeight", true);
        strategy.update_success_rate("MinimizeWeight", true);
        strategy.update_success_rate("MinimizeWeight", false);

        // Check learned rate
        let rate = strategy.success_rates.get("MinimizeWeight").unwrap();
        assert!(*rate > 0.5); // Should be positive due to more successes
    }

    #[test]
    fn test_variance_calculation() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let variance = calculate_variance(&values);
        assert!((variance - 2.0).abs() < 0.01); // Variance should be 2.0
    }
}
