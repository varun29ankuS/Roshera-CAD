//! Context builder for rich AI understanding
//!
//! This module builds comprehensive context from session state, user history,
//! and scene information to provide better AI responses.

use crate::providers::{CommandIntent, ConversationContext, ParsedCommand};
use serde_json::Value;
use session_manager::{PermissionManager, SessionManager, UserPermissions};
use shared_types::{
    AICommand, CADObject, GeometryId, HistoryEntry, MaterialRef, ObjectId, ObjectType,
    SceneBoundingBox, SceneObject, SceneState, SceneTransform3D, SessionState,
};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tracing::{debug, info};

/// Rich context for AI operations
#[derive(Debug, Clone)]
pub struct RichAIContext {
    /// Base conversation context
    pub conversation: ConversationContext,
    /// Scene analysis
    pub scene_analysis: SceneAnalysis,
    /// User context
    pub user_context: UserContext,
    /// Collaboration context
    pub collaboration_context: CollaborationContext,
    /// Suggestions based on context
    pub suggestions: Vec<ContextualSuggestion>,
}

/// Scene analysis results
#[derive(Debug, Clone)]
pub struct SceneAnalysis {
    /// Total number of objects
    pub object_count: usize,
    /// Object type distribution
    pub object_types: HashMap<String, usize>,
    /// Scene bounding box
    pub bounds: SceneBoundingBox,
    /// Material usage
    pub materials: HashMap<String, usize>,
    /// Complexity score (0-1)
    pub complexity_score: f64,
    /// Spatial relationships
    pub relationships: Vec<SpatialRelationship>,
    /// Identified patterns
    pub patterns: Vec<ScenePattern>,
}

/// User context information
#[derive(Debug, Clone)]
pub struct UserContext {
    /// User ID
    pub user_id: String,
    /// User role and permissions
    pub permissions: UserPermissions,
    /// Recent command history
    pub recent_commands: Vec<String>,
    /// User preferences
    pub preferences: UserPreferences,
    /// Skill level estimation
    pub skill_level: SkillLevel,
}

/// User preferences
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UserPreferences {
    /// Preferred units (mm, cm, m, in, ft)
    pub units: String,
    /// Language preference
    pub language: String,
    /// AI assistance level
    pub ai_assistance_level: String,
    /// Default materials
    pub default_materials: Vec<String>,
    /// Preferred workflows
    pub preferred_workflows: Vec<String>,
}

/// User skill level
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SkillLevel {
    Beginner,
    Intermediate,
    Advanced,
    Expert,
}

/// Collaboration context
#[derive(Debug, Clone)]
pub struct CollaborationContext {
    /// Active users in session
    pub active_users: Vec<UserInfo>,
    /// Recent collaborative actions
    pub recent_actions: Vec<CollaborativeAction>,
    /// Locked objects (user_id -> object_ids)
    pub locked_objects: HashMap<String, Vec<ObjectId>>,
    /// Active selections by users
    pub user_selections: HashMap<String, Vec<ObjectId>>,
}

/// User info for collaboration
#[derive(Debug, Clone)]
pub struct UserInfo {
    pub id: String,
    pub name: String,
    pub role: String,
    pub is_active: bool,
    pub last_activity: u64,
}

/// Collaborative action
#[derive(Debug, Clone)]
pub struct CollaborativeAction {
    pub user_id: String,
    pub action: String,
    pub target_objects: Vec<GeometryId>,
    pub timestamp: u64,
}

/// Spatial relationship between objects
#[derive(Debug, Clone)]
pub struct SpatialRelationship {
    pub object_a: ObjectId,
    pub object_b: ObjectId,
    pub relationship_type: RelationshipType,
    pub distance: Option<f64>,
}

#[derive(Debug, Clone)]
pub enum RelationshipType {
    Adjacent,
    Overlapping,
    Inside,
    Aligned,
    Parallel,
    Perpendicular,
}

/// Identified patterns in the scene
#[derive(Debug, Clone)]
pub enum ScenePattern {
    /// Linear array of objects
    LinearArray {
        objects: Vec<ObjectId>,
        spacing: f64,
    },
    /// Circular pattern
    CircularPattern {
        objects: Vec<ObjectId>,
        center: [f64; 3],
        radius: f64,
    },
    /// Symmetry
    Symmetry {
        axis: [f64; 3],
        object_pairs: Vec<(ObjectId, ObjectId)>,
    },
    /// Assembly
    Assembly {
        parent: GeometryId,
        children: Vec<GeometryId>,
    },
}

/// Contextual suggestion
#[derive(Debug, Clone)]
pub struct ContextualSuggestion {
    /// Suggestion text
    pub text: String,
    /// Example command
    pub example_command: String,
    /// Relevance score (0-1)
    pub relevance: f64,
    /// Category
    pub category: SuggestionCategory,
}

#[derive(Debug, Clone)]
pub enum SuggestionCategory {
    NextStep,
    Correction,
    Optimization,
    Pattern,
    Collaboration,
}

/// Context builder
pub struct ContextBuilder {
    session_manager: Arc<SessionManager>,
}

impl ContextBuilder {
    /// Create new context builder
    pub fn new(session_manager: Arc<SessionManager>) -> Self {
        Self { session_manager }
    }

    /// Build rich context for AI operations
    pub async fn build_context(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Result<RichAIContext, Box<dyn std::error::Error + Send + Sync>> {
        debug!(
            "Building rich AI context for session {} user {}",
            session_id, user_id
        );

        // Get session state
        let session = self.session_manager.get_session(session_id).await?;
        let session_state = session.read().await;

        // Build scene analysis
        let scene_analysis = self.analyze_scene(&session_state);

        // Build user context
        let user_context = self.build_user_context(user_id, session_id).await?;

        // Build collaboration context
        let collaboration_context = self.build_collaboration_context(&session_state);

        // Generate suggestions
        let suggestions =
            self.generate_suggestions(&scene_analysis, &user_context, &collaboration_context);

        // Build conversation context. The session's command history (a
        // VecDeque of `HistoryEntry`) is structured (`AICommand`) rather than
        // free-form text; we project the most recent N entries onto the
        // provider-side `ParsedCommand` shape so downstream prompts (Claude,
        // OpenAI) can ground continuations on prior intent. The cap matches
        // the limit applied in `processor.rs` (10).
        const MAX_PREVIOUS_COMMANDS: usize = 10;
        let previous_commands: Vec<ParsedCommand> = session_state
            .history
            .iter()
            .rev()
            .take(MAX_PREVIOUS_COMMANDS)
            .map(history_entry_to_parsed_command)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();

        let conversation = ConversationContext {
            session_id: session_id.to_string(),
            previous_commands,
            active_objects: session_state
                .objects
                .keys()
                .map(|id| id.to_string())
                .collect(),
            user_preferences: serde_json::to_value(&user_context.preferences)?,
            scene_state: None,    // Scene state is a different type
            system_context: None, // SystemContext has different fields
        };

        Ok(RichAIContext {
            conversation,
            scene_analysis,
            user_context,
            collaboration_context,
            suggestions,
        })
    }

    /// Analyze scene
    fn analyze_scene(&self, session_state: &SessionState) -> SceneAnalysis {
        let mut object_types = HashMap::new();
        let mut materials = HashMap::new();
        let mut all_bounds = Vec::new();

        // Analyze objects
        for object in session_state.objects.values() {
            *object_types.entry("CADObject".to_string()).or_insert(0) += 1;

            *materials.entry(object.material.name.clone()).or_insert(0) += 1;

            // Collect bounds (simplified)
            all_bounds.push(self.estimate_object_bounds(object));
        }

        // Calculate scene bounds
        let bounds = self.calculate_combined_bounds(&all_bounds);

        // Detect relationships
        let relationships = self.detect_relationships(&session_state.objects);

        // Detect patterns
        let patterns = self.detect_patterns(&session_state.objects);

        // Calculate complexity
        let complexity_score =
            self.calculate_complexity_score(session_state.objects.len(), &relationships, &patterns);

        SceneAnalysis {
            object_count: session_state.objects.len(),
            object_types,
            bounds,
            materials,
            complexity_score,
            relationships,
            patterns,
        }
    }

    /// Build user context
    async fn build_user_context(
        &self,
        user_id: &str,
        session_id: &str,
    ) -> Result<UserContext, Box<dyn std::error::Error + Send + Sync>> {
        // Get permissions (simplified - use default permissions)
        let permissions = session_manager::UserPermissions {
            user_id: user_id.to_string(),
            role: session_manager::Role::Editor,
            explicit_permissions: std::collections::HashSet::new(),
            denied_permissions: std::collections::HashSet::new(),
            granted_by: user_id.to_string(), // Self-granted for demo
            updated_at: chrono::Utc::now(),
        };

        // Get recent commands (simplified - in real impl, query from timeline)
        let recent_commands = vec![
            "create a box with width 10".to_string(),
            "add a cylinder".to_string(),
        ];

        // Load preferences (simplified - in real impl, load from database)
        let preferences = UserPreferences {
            units: "mm".to_string(),
            language: "en".to_string(),
            ai_assistance_level: "intermediate".to_string(),
            default_materials: vec!["steel".to_string(), "aluminum".to_string()],
            preferred_workflows: vec!["mechanical".to_string()],
        };

        // Estimate skill level based on command history
        let skill_level = self.estimate_skill_level(&recent_commands);

        Ok(UserContext {
            user_id: user_id.to_string(),
            permissions,
            recent_commands,
            preferences,
            skill_level,
        })
    }

    /// Build collaboration context
    fn build_collaboration_context(&self, session_state: &SessionState) -> CollaborationContext {
        let active_users: Vec<UserInfo> = session_state
            .active_users
            .iter()
            .map(|u| UserInfo {
                id: u.id.clone(),
                name: u.name.clone(),
                role: format!("{:?}", u.role),
                is_active: u.is_active(30000), // 30 second timeout
                last_activity: u.last_activity,
            })
            .collect();

        // Mock recent actions - in real impl, track these
        let recent_actions = vec![CollaborativeAction {
            user_id: "user1".to_string(),
            action: "created box".to_string(),
            target_objects: vec![],
            timestamp: chrono::Utc::now().timestamp() as u64 - 60,
        }];

        // Mock locked objects - in real impl, track from permission system
        let locked_objects = HashMap::new();
        let user_selections = HashMap::new();

        CollaborationContext {
            active_users,
            recent_actions,
            locked_objects,
            user_selections,
        }
    }

    /// Generate contextual suggestions
    fn generate_suggestions(
        &self,
        scene: &SceneAnalysis,
        user: &UserContext,
        collaboration: &CollaborationContext,
    ) -> Vec<ContextualSuggestion> {
        let mut suggestions = Vec::new();

        // Suggest based on scene state
        if scene.object_count == 0 {
            suggestions.push(ContextualSuggestion {
                text: "Start by creating a basic shape".to_string(),
                example_command: "create a box with width 100mm".to_string(),
                relevance: 1.0,
                category: SuggestionCategory::NextStep,
            });
        } else if scene.object_count == 2 {
            suggestions.push(ContextualSuggestion {
                text: "Try combining objects with boolean operations".to_string(),
                example_command: "union the two objects".to_string(),
                relevance: 0.9,
                category: SuggestionCategory::NextStep,
            });
        }

        // Suggest based on patterns
        for pattern in &scene.patterns {
            match pattern {
                ScenePattern::LinearArray {
                    objects: _,
                    spacing,
                } => {
                    suggestions.push(ContextualSuggestion {
                        text: format!("Continue the linear array pattern (spacing: {}mm)", spacing),
                        example_command: "add another object to the array".to_string(),
                        relevance: 0.8,
                        category: SuggestionCategory::Pattern,
                    });
                }
                _ => {}
            }
        }

        // Suggest based on skill level
        match user.skill_level {
            SkillLevel::Beginner => {
                suggestions.push(ContextualSuggestion {
                    text: "Try using the measurement tool to check dimensions".to_string(),
                    example_command: "measure the distance between objects".to_string(),
                    relevance: 0.7,
                    category: SuggestionCategory::NextStep,
                });
            }
            SkillLevel::Advanced | SkillLevel::Expert => {
                suggestions.push(ContextualSuggestion {
                    text: "Consider using parametric constraints".to_string(),
                    example_command: "add a tangent constraint".to_string(),
                    relevance: 0.6,
                    category: SuggestionCategory::Optimization,
                });
            }
            _ => {}
        }

        // Collaboration suggestions
        if collaboration.active_users.len() > 1 {
            suggestions.push(ContextualSuggestion {
                text: "Coordinate with other users on the design".to_string(),
                example_command: "highlight my work area".to_string(),
                relevance: 0.5,
                category: SuggestionCategory::Collaboration,
            });
        }

        suggestions
    }

    /// Estimate object bounds
    fn estimate_object_bounds(&self, _object: &CADObject) -> SceneBoundingBox {
        // Simplified - in real impl, calculate from geometry
        SceneBoundingBox {
            min: [-50.0, -50.0, -50.0],
            max: [50.0, 50.0, 50.0],
        }
    }

    /// Calculate combined bounds
    fn calculate_combined_bounds(&self, bounds: &[SceneBoundingBox]) -> SceneBoundingBox {
        if bounds.is_empty() {
            return SceneBoundingBox {
                min: [0.0, 0.0, 0.0],
                max: [0.0, 0.0, 0.0],
            };
        }

        let mut min = bounds[0].min;
        let mut max = bounds[0].max;

        for b in bounds.iter().skip(1) {
            for i in 0..3 {
                min[i] = min[i].min(b.min[i]);
                max[i] = max[i].max(b.max[i]);
            }
        }

        SceneBoundingBox { min, max }
    }

    /// Detect spatial relationships
    fn detect_relationships(
        &self,
        objects: &HashMap<ObjectId, CADObject>,
    ) -> Vec<SpatialRelationship> {
        let mut relationships = Vec::new();

        // Simplified - detect adjacent objects
        let object_vec: Vec<_> = objects.iter().collect();
        for i in 0..object_vec.len() {
            for j in i + 1..object_vec.len() {
                let (id_a, _obj_a) = object_vec[i];
                let (id_b, _obj_b) = object_vec[j];

                // Simplified adjacency check
                relationships.push(SpatialRelationship {
                    object_a: id_a.clone(),
                    object_b: id_b.clone(),
                    relationship_type: RelationshipType::Adjacent,
                    distance: Some(10.0), // Mock distance
                });
            }
        }

        relationships
    }

    /// Detect patterns in scene
    fn detect_patterns(&self, objects: &HashMap<ObjectId, CADObject>) -> Vec<ScenePattern> {
        let mut patterns = Vec::new();

        // Simplified pattern detection
        if objects.len() >= 3 {
            // Check for linear array (simplified)
            let ids: Vec<_> = objects.keys().cloned().collect();
            if ids.len() >= 3 {
                patterns.push(ScenePattern::LinearArray {
                    objects: ids[..3].to_vec(),
                    spacing: 50.0, // Mock spacing
                });
            }
        }

        patterns
    }

    /// Calculate scene complexity score
    fn calculate_complexity_score(
        &self,
        object_count: usize,
        relationships: &[SpatialRelationship],
        patterns: &[ScenePattern],
    ) -> f64 {
        let base_score = (object_count as f64).ln() / 10.0;
        let relationship_score = (relationships.len() as f64) / 100.0;
        let pattern_score = (patterns.len() as f64) / 10.0;

        (base_score + relationship_score + pattern_score).min(1.0)
    }

    /// Estimate user skill level
    fn estimate_skill_level(&self, recent_commands: &[String]) -> SkillLevel {
        // Simplified heuristic
        let advanced_keywords = ["constraint", "parametric", "nurbs", "boolean"];
        let advanced_count = recent_commands
            .iter()
            .filter(|cmd| advanced_keywords.iter().any(|k| cmd.contains(k)))
            .count();

        match advanced_count {
            0 => SkillLevel::Beginner,
            1..=2 => SkillLevel::Intermediate,
            3..=5 => SkillLevel::Advanced,
            _ => SkillLevel::Expert,
        }
    }
}

/// Project a `HistoryEntry` (whose `command` field is a structured
/// `AICommand`) onto the provider-side `ParsedCommand` shape used by
/// `ConversationContext::previous_commands`.
///
/// The mapping is lossless on the structural side: the full `AICommand`
/// JSON is preserved in `ParsedCommand::parameters["command"]`. The
/// `intent` is collapsed onto `CommandIntent` so providers can branch on
/// the high-level intent without re-deserializing the parameters. The
/// `original_text` is reconstructed from `HistoryEntry::description`,
/// which is the human-readable label captured when the entry was logged.
fn history_entry_to_parsed_command(entry: &HistoryEntry) -> ParsedCommand {
    let intent = match &entry.command {
        AICommand::CreatePrimitive { shape_type, .. } => CommandIntent::CreatePrimitive {
            shape: format!("{:?}", shape_type),
        },
        AICommand::BooleanOperation { operation, .. } => CommandIntent::BooleanOperation {
            operation: format!("{:?}", operation),
        },
        AICommand::Transform { transform_type, .. } => CommandIntent::Transform {
            operation: format!("{:?}", transform_type),
        },
        AICommand::ChangeView { view_type } => CommandIntent::Query {
            target: format!("view::{:?}", view_type),
        },
        AICommand::ModifyMaterial { object_id, .. } => CommandIntent::Modify {
            target: object_id.to_string(),
            operation: "material".to_string(),
            parameters: serde_json::to_value(&entry.command).unwrap_or(Value::Null),
        },
        AICommand::Export { format, .. } => CommandIntent::Export {
            format: format!("{:?}", format),
            options: serde_json::to_value(&entry.command).unwrap_or(Value::Null),
        },
        AICommand::SessionControl { action } => CommandIntent::Query {
            target: format!("session::{:?}", action),
        },
        AICommand::Analyze { analysis_type, .. } => CommandIntent::Query {
            target: format!("analyze::{:?}", analysis_type),
        },
    };

    let mut parameters = HashMap::new();
    parameters.insert(
        "command".to_string(),
        serde_json::to_value(&entry.command).unwrap_or(Value::Null),
    );
    if let Some(user) = &entry.user_id {
        parameters.insert("user_id".to_string(), Value::String(user.clone()));
    }
    parameters.insert(
        "timestamp".to_string(),
        serde_json::to_value(entry.timestamp).unwrap_or(Value::Null),
    );

    ParsedCommand {
        original_text: entry.description.clone(),
        intent,
        parameters,
        confidence: 1.0,
        language: "en".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_context_builder() {
        // Test implementation
    }
}
