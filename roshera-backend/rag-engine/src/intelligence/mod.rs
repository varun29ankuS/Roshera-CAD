//! Intelligence engine for user-specific learning and intent classification
//!
//! Provides:
//! - User expertise tracking
//! - Intent classification
//! - Personalized context generation
//! - Team knowledge sharing

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Main intelligence engine
#[derive(Clone)]
pub struct IntelligenceEngine {
    user_profiles: Arc<DashMap<uuid::Uuid, UserProfile>>,
    intent_classifier: Arc<IntentClassifier>,
    context_builder: Arc<ContextBuilder>,
    team_knowledge: Arc<TeamKnowledge>,
}

/// User learning and personalization
pub struct UserLearning {
    profiles: Arc<DashMap<uuid::Uuid, UserProfile>>,
    behavior_analyzer: BehaviorAnalyzer,
    preference_tracker: PreferenceTracker,
}

/// User profile with expertise and preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: uuid::Uuid,
    pub expertise_level: ExpertiseLevel,
    pub preferred_workflows: Vec<Workflow>,
    pub common_operations: Vec<OperationPattern>,
    pub error_patterns: Vec<ErrorPattern>,
    pub learning_history: LearningHistory,
    pub team_id: Option<uuid::Uuid>,
}

/// User expertise level
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ExpertiseLevel {
    Beginner {
        hours_used: f64,
        tutorials_completed: Vec<String>,
    },
    Intermediate {
        projects_completed: usize,
        features_mastered: Vec<String>,
    },
    Advanced {
        complex_operations: Vec<String>,
        custom_workflows: Vec<Workflow>,
    },
    Expert {
        contributions: Vec<Contribution>,
        specializations: Vec<String>,
    },
}

/// Workflow pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workflow {
    pub name: String,
    pub steps: Vec<WorkflowStep>,
    pub frequency: u32,
    pub success_rate: f32,
}

/// Workflow step
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub operation: String,
    pub parameters: serde_json::Value,
    pub average_duration: std::time::Duration,
}

/// Operation pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationPattern {
    pub operation_type: String,
    pub frequency: u32,
    pub typical_parameters: serde_json::Value,
    pub success_rate: f32,
}

/// Error pattern
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPattern {
    pub error_type: String,
    pub frequency: u32,
    pub recovery_method: Option<String>,
    pub prevention_suggestion: String,
}

/// Learning history
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningHistory {
    pub sessions: Vec<SessionSummary>,
    pub total_hours: f64,
    pub growth_rate: f32,
    pub milestones: Vec<Milestone>,
}

/// Session summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSummary {
    pub session_id: uuid::Uuid,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub duration: std::time::Duration,
    pub operations_performed: u32,
    pub new_concepts_learned: Vec<String>,
}

/// Learning milestone
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Milestone {
    pub name: String,
    pub achieved_at: chrono::DateTime<chrono::Utc>,
    pub achievement_type: AchievementType,
}

/// Achievement type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AchievementType {
    FirstOperation(String),
    MasteredFeature(String),
    CompletedProject,
    ReachedExpertise(String),
}

/// User contribution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Contribution {
    pub contribution_type: String,
    pub impact_score: f32,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// Intent classifier
pub struct IntentClassifier {
    patterns: HashMap<String, IntentPattern>,
    ml_model: Option<IntentModel>,
}

/// Intent pattern
#[derive(Debug, Clone)]
pub struct IntentPattern {
    pub intent: Intent,
    pub keywords: Vec<String>,
    pub regex_patterns: Vec<regex::Regex>,
    pub confidence_threshold: f32,
}

/// User intent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Intent {
    CreateGeometry { geometry_type: String },
    ModifyGeometry { operation: String },
    QueryInformation { topic: String },
    LearnConcept { concept: String },
    ReportIssue { issue_type: String },
    RequestHelp { context: String },
    PerformAnalysis { analysis_type: String },
    ExportDesign { format: String },
}

/// Intent classification model
struct IntentModel {
    // Would contain actual ML model
}

/// Context builder for RAG
pub struct ContextBuilder {
    user_context: UserContextBuilder,
    project_context: ProjectContextBuilder,
    system_context: SystemContextBuilder,
}

/// User context builder
struct UserContextBuilder;

/// Project context builder
struct ProjectContextBuilder;

/// System context builder
struct SystemContextBuilder;

/// Team knowledge sharing
pub struct TeamKnowledge {
    shared_workflows: Arc<DashMap<String, Workflow>>,
    best_practices: Arc<DashMap<String, BestPractice>>,
    common_solutions: Arc<DashMap<String, Solution>>,
}

/// Best practice
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BestPractice {
    pub title: String,
    pub description: String,
    pub category: String,
    pub votes: u32,
    pub author: uuid::Uuid,
}

/// Common solution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Solution {
    pub problem: String,
    pub solution: String,
    pub success_rate: f32,
    pub usage_count: u32,
}

/// Behavior analyzer
struct BehaviorAnalyzer {
    pattern_detector: PatternDetector,
    anomaly_detector: AnomalyDetector,
}

/// Pattern detector
struct PatternDetector;

/// Anomaly detector
struct AnomalyDetector;

/// Preference tracker
struct PreferenceTracker {
    ui_preferences: HashMap<uuid::Uuid, UIPreferences>,
    workflow_preferences: HashMap<uuid::Uuid, WorkflowPreferences>,
}

/// UI preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
struct UIPreferences {
    theme: String,
    layout: String,
    shortcuts: HashMap<String, String>,
}

/// Workflow preferences
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowPreferences {
    auto_save: bool,
    validation_level: String,
    default_units: String,
}

impl IntelligenceEngine {
    /// Create new intelligence engine
    pub fn new(config: &crate::IntelligenceConfig) -> anyhow::Result<Self> {
        Ok(Self {
            user_profiles: Arc::new(DashMap::new()),
            intent_classifier: Arc::new(IntentClassifier::new()),
            context_builder: Arc::new(ContextBuilder::new()),
            team_knowledge: Arc::new(TeamKnowledge::new()),
        })
    }

    /// Get or create user profile
    pub async fn get_user_profile(&self, user_id: uuid::Uuid) -> UserProfile {
        self.user_profiles
            .entry(user_id)
            .or_insert_with(|| UserProfile::new(user_id))
            .clone()
    }

    /// Update user profile based on session
    pub async fn update_profile(&self, user_id: uuid::Uuid, session: &crate::Session) {
        if let Some(mut profile) = self.user_profiles.get_mut(&user_id) {
            // Update expertise level
            profile.update_from_session(session);
            
            // Track operations
            for op in &session.operations {
                profile.track_operation(op);
            }
            
            // Detect patterns
            profile.detect_patterns();
        }
    }

    /// Classify user intent
    pub async fn classify_intent(&self, query: &str, user_id: uuid::Uuid) -> Intent {
        // Get user profile for context
        let profile = self.get_user_profile(user_id).await;
        
        // Classify based on patterns and user expertise
        self.intent_classifier.classify(query, &profile)
    }

    /// Build context for RAG
    pub async fn build_context(
        &self,
        query: &str,
        user_id: uuid::Uuid,
    ) -> anyhow::Result<EnhancedContext> {
        let profile = self.get_user_profile(user_id).await;
        let intent = self.classify_intent(query, user_id).await;
        
        // Get team knowledge before moving profile fields
        let team_knowledge = self.get_team_knowledge(&profile).await;
        
        Ok(EnhancedContext {
            user_expertise: profile.expertise_level,
            intent,
            relevant_workflows: profile.preferred_workflows,
            team_knowledge,
            system_capabilities: self.context_builder.get_system_context(),
        })
    }

    /// Get relevant team knowledge
    async fn get_team_knowledge(&self, profile: &UserProfile) -> Vec<TeamKnowledgeItem> {
        let mut items = Vec::new();
        
        if let Some(team_id) = profile.team_id {
            // Get shared workflows
            for workflow in self.team_knowledge.shared_workflows.iter() {
                items.push(TeamKnowledgeItem::Workflow(workflow.value().clone()));
            }
            
            // Get best practices
            for practice in self.team_knowledge.best_practices.iter() {
                items.push(TeamKnowledgeItem::BestPractice(practice.value().clone()));
            }
        }
        
        items
    }
}

impl UserProfile {
    pub fn new(user_id: uuid::Uuid) -> Self {
        Self {
            user_id,
            expertise_level: ExpertiseLevel::Beginner {
                hours_used: 0.0,
                tutorials_completed: Vec::new(),
            },
            preferred_workflows: Vec::new(),
            common_operations: Vec::new(),
            error_patterns: Vec::new(),
            learning_history: LearningHistory {
                sessions: Vec::new(),
                total_hours: 0.0,
                growth_rate: 0.0,
                milestones: Vec::new(),
            },
            team_id: None,
        }
    }

    pub fn update_from_session(&mut self, session: &crate::Session) {
        // Update learning history
        self.learning_history.sessions.push(SessionSummary {
            session_id: uuid::Uuid::new_v4(),
            timestamp: chrono::Utc::now(),
            duration: session.duration,
            operations_performed: session.operations.len() as u32,
            new_concepts_learned: Vec::new(),
        });
        
        // Update total hours
        self.learning_history.total_hours += session.duration.as_secs_f64() / 3600.0;
        
        // Update expertise level
        self.update_expertise_level();
    }

    pub fn track_operation(&mut self, operation: &crate::Operation) {
        // Find or create operation pattern
        let pattern = self.common_operations
            .iter_mut()
            .find(|p| p.operation_type == operation.op_type);
        
        if let Some(pattern) = pattern {
            pattern.frequency += 1;
        } else {
            self.common_operations.push(OperationPattern {
                operation_type: operation.op_type.clone(),
                frequency: 1,
                typical_parameters: operation.params.clone(),
                success_rate: 1.0,
            });
        }
    }

    pub fn detect_patterns(&mut self) {
        // Sort operations by frequency
        self.common_operations.sort_by(|a, b| b.frequency.cmp(&a.frequency));
        
        // Detect workflow patterns
        // This would use sequence mining algorithms
    }

    fn update_expertise_level(&mut self) {
        let hours = self.learning_history.total_hours;
        
        self.expertise_level = if hours < 10.0 {
            ExpertiseLevel::Beginner {
                hours_used: hours,
                tutorials_completed: Vec::new(),
            }
        } else if hours < 100.0 {
            ExpertiseLevel::Intermediate {
                projects_completed: (hours / 10.0) as usize,
                features_mastered: Vec::new(),
            }
        } else if hours < 1000.0 {
            ExpertiseLevel::Advanced {
                complex_operations: Vec::new(),
                custom_workflows: Vec::new(),
            }
        } else {
            ExpertiseLevel::Expert {
                contributions: Vec::new(),
                specializations: Vec::new(),
            }
        };
    }
}

impl IntentClassifier {
    pub fn new() -> Self {
        Self {
            patterns: Self::init_patterns(),
            ml_model: None,
        }
    }

    fn init_patterns() -> HashMap<String, IntentPattern> {
        let mut patterns = HashMap::new();
        
        // Create geometry patterns
        patterns.insert(
            "create_geometry".to_string(),
            IntentPattern {
                intent: Intent::CreateGeometry {
                    geometry_type: String::new(),
                },
                keywords: vec!["create".to_string(), "make".to_string(), "add".to_string(), "draw".to_string(), "design".to_string()],
                regex_patterns: vec![],
                confidence_threshold: 0.7,
            },
        );
        
        // More patterns would be added here
        
        patterns
    }

    pub fn classify(&self, query: &str, profile: &UserProfile) -> Intent {
        // Simple keyword matching for now
        let query_lower = query.to_lowercase();
        
        for pattern in self.patterns.values() {
            for keyword in &pattern.keywords {
                if query_lower.contains(keyword) {
                    return pattern.intent.clone();
                }
            }
        }
        
        // Default to query information
        Intent::QueryInformation {
            topic: "general".to_string(),
        }
    }
}

impl ContextBuilder {
    pub fn new() -> Self {
        Self {
            user_context: UserContextBuilder,
            project_context: ProjectContextBuilder,
            system_context: SystemContextBuilder,
        }
    }

    pub fn get_system_context(&self) -> SystemContext {
        SystemContext {
            capabilities: vec![
                "CAD modeling".to_string(),
                "Boolean operations".to_string(),
                "NURBS surfaces".to_string(),
            ],
            version: "0.1.0".to_string(),
        }
    }
}

impl TeamKnowledge {
    pub fn new() -> Self {
        Self {
            shared_workflows: Arc::new(DashMap::new()),
            best_practices: Arc::new(DashMap::new()),
            common_solutions: Arc::new(DashMap::new()),
        }
    }
}

impl UserLearning {
    pub fn new(profiles: Arc<DashMap<uuid::Uuid, UserProfile>>) -> Self {
        Self {
            profiles,
            behavior_analyzer: BehaviorAnalyzer {
                pattern_detector: PatternDetector,
                anomaly_detector: AnomalyDetector,
            },
            preference_tracker: PreferenceTracker {
                ui_preferences: HashMap::new(),
                workflow_preferences: HashMap::new(),
            },
        }
    }
}

/// Enhanced context for RAG
#[derive(Debug, Clone)]
pub struct EnhancedContext {
    pub user_expertise: ExpertiseLevel,
    pub intent: Intent,
    pub relevant_workflows: Vec<Workflow>,
    pub team_knowledge: Vec<TeamKnowledgeItem>,
    pub system_capabilities: SystemContext,
}

/// Team knowledge item
#[derive(Debug, Clone)]
pub enum TeamKnowledgeItem {
    Workflow(Workflow),
    BestPractice(BestPractice),
    Solution(Solution),
}

/// System context
#[derive(Debug, Clone)]
pub struct SystemContext {
    pub capabilities: Vec<String>,
    pub version: String,
}