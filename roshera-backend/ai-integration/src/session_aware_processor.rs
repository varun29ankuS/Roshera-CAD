//! Session-aware AI processor with authentication, caching, and permissions
//!
//! This module provides an enhanced AI processor that integrates with
//! session-manager's advanced features.

use crate::executor::CommandExecutor;
use shared_types::{CommandResult, GeometryId}; // Hybrid architecture: CommandResult moved to shared-types for API consistency

// Helper functions for creating CommandResult instances - consistent with processor.rs
// Architecture: Accept impl Into<String> for flexibility with &str and String types
fn create_success_result_with_objects(
    message: impl Into<String>,
    object_ids: Vec<GeometryId>,
) -> CommandResult {
    let mut result = CommandResult::success(message.into());
    if !object_ids.is_empty() {
        result.object_id = object_ids.first().copied(); // Set primary object
                                                        // Convert GeometryId to ObjectId for objects_affected field
        result.objects_affected = object_ids.iter().map(|id| id.0).collect();
    }
    result
}

fn create_query_result(message: impl Into<String>, data: serde_json::Value) -> CommandResult {
    let mut result = CommandResult::success(message.into());
    result.data = Some(data);
    result
}

fn create_error_result(message: impl Into<String>) -> CommandResult {
    let mut result = CommandResult::success("Command failed");
    result.success = false;
    result.error = Some(message.into());
    result
}
use crate::providers::{
    AudioFormat, CommandIntent, ConversationContext, ParsedCommand, ProviderManager,
};
use crate::{Operation, ProcessedCommand, VoiceCommand};
use serde::{Deserialize, Serialize};
use session_manager::{
    auth::AuthManager,
    cache::CacheManager,
    permissions::PermissionManager,
    permissions::{Permission, UserPermissions},
    SessionManager,
};
use shared_types::SessionError;
use shared_types::{Command, ObjectId, PrimitiveType, ShapeParameters};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Authentication context for AI operations
#[derive(Debug, Clone)]
pub struct AIAuthContext {
    /// User ID from JWT token
    pub user_id: String,
    /// Session ID
    pub session_id: String,
    /// User permissions
    pub permissions: UserPermissions,
    /// API key ID (if using API key auth)
    pub api_key_id: Option<String>,
}

/// Session-aware AI processor configuration
#[derive(Debug, Clone)]
pub struct SessionAwareConfig {
    /// Enable caching of AI responses
    pub enable_caching: bool,
    /// Cache TTL for AI responses in seconds
    pub cache_ttl_seconds: u64,
    /// Enable real-time broadcasting
    pub enable_broadcasting: bool,
    /// Require permissions for operations
    pub enforce_permissions: bool,
    /// Log all AI operations for audit
    pub enable_audit_log: bool,
}

impl Default for SessionAwareConfig {
    fn default() -> Self {
        Self {
            enable_caching: true,
            cache_ttl_seconds: 300, // 5 minutes
            enable_broadcasting: true,
            enforce_permissions: true,
            enable_audit_log: true,
        }
    }
}

/// Enhanced AI processor with session management integration
pub struct SessionAwareAIProcessor {
    /// Base provider manager
    provider_manager: Arc<Mutex<ProviderManager>>,
    /// Command executor
    executor: Arc<Mutex<CommandExecutor>>,
    /// Session manager
    session_manager: Arc<SessionManager>,
    /// Configuration
    config: SessionAwareConfig,
    /// Active auth contexts
    auth_contexts: Arc<RwLock<HashMap<String, AIAuthContext>>>,
}

impl SessionAwareAIProcessor {
    /// Create new session-aware AI processor
    pub fn new(
        provider_manager: Arc<Mutex<ProviderManager>>,
        executor: Arc<Mutex<CommandExecutor>>,
        session_manager: Arc<SessionManager>,
        config: SessionAwareConfig,
    ) -> Self {
        Self {
            provider_manager,
            executor,
            session_manager,
            config,
            auth_contexts: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Authenticate user and create auth context
    pub async fn authenticate(
        &self,
        token: &str,
    ) -> Result<AIAuthContext, Box<dyn std::error::Error + Send + Sync>> {
        // Verify JWT token
        let auth_manager = self.session_manager.auth_manager();
        let claims = auth_manager.verify_token(token)?;

        // Get user permissions - for now, use default permissions
        let permissions = UserPermissions {
            user_id: claims.sub.clone(),
            role: session_manager::Role::Editor, // Default role
            explicit_permissions: HashSet::new(),
            denied_permissions: HashSet::new(),
            updated_at: chrono::Utc::now(),
            granted_by: "system".to_string(),
        };

        let auth_context = AIAuthContext {
            user_id: claims.sub.clone(),
            session_id: claims.sub, // In real app, extract from claims
            permissions,
            api_key_id: None,
        };

        // Store auth context
        let mut contexts = self.auth_contexts.write().await;
        contexts.insert(token.to_string(), auth_context.clone());

        Ok(auth_context)
    }

    /// Authenticate with API key
    pub async fn authenticate_api_key(
        &self,
        api_key: &str,
    ) -> Result<AIAuthContext, Box<dyn std::error::Error + Send + Sync>> {
        let auth_manager = self.session_manager.auth_manager();
        let key_info = auth_manager.verify_api_key(api_key)?;

        // Get permissions from API key
        let permissions = UserPermissions {
            user_id: key_info.user_id.clone(),
            role: session_manager::Role::Custom(0), // API role
            explicit_permissions: HashSet::new(),   // For now, empty
            denied_permissions: HashSet::new(),
            updated_at: chrono::Utc::now(),
            granted_by: key_info.user_id.clone(),
        };

        let auth_context = AIAuthContext {
            user_id: key_info.user_id.clone(),
            session_id: Uuid::new_v4().to_string(), // Generate session for API
            permissions,
            api_key_id: Some(key_info.id),
        };

        // Store auth context
        let mut contexts = self.auth_contexts.write().await;
        contexts.insert(api_key.to_string(), auth_context.clone());

        Ok(auth_context)
    }

    /// Process voice input with full session awareness
    pub async fn process_voice_with_session(
        &self,
        auth_token: &str,
        audio: &[u8],
        format: AudioFormat,
    ) -> Result<ProcessedCommand, Box<dyn std::error::Error + Send + Sync>> {
        let start = std::time::Instant::now();

        // Get auth context
        let auth_context = {
            let contexts = self.auth_contexts.read().await;
            contexts
                .get(auth_token)
                .cloned()
                .ok_or("Authentication required")?
        };

        info!(
            "Processing voice command for user {} in session {}",
            auth_context.user_id, auth_context.session_id
        );

        // Check if cached response exists
        if self.config.enable_caching {
            let cache_key = self.compute_audio_cache_key(audio);
            if let Some(cached) = self.get_cached_response(&cache_key).await {
                debug!("Returning cached AI response");
                return Ok(cached);
            }
        }

        // Step 1: Speech to text
        let text = {
            let manager = self.provider_manager.lock().await;
            let asr = manager.asr()?;
            asr.transcribe(audio, format).await?
        };

        // Process text with session context
        self.process_text_with_session(auth_token, &text).await
    }

    /// Process text input with session awareness
    pub async fn process_text_with_session(
        &self,
        auth_token: &str,
        text: &str,
    ) -> Result<ProcessedCommand, Box<dyn std::error::Error + Send + Sync>> {
        let start = std::time::Instant::now();

        // Get auth context
        let auth_context = {
            let contexts = self.auth_contexts.read().await;
            contexts
                .get(auth_token)
                .cloned()
                .ok_or("Authentication required")?
        };

        // Check cache
        if self.config.enable_caching {
            let cache_key = format!("text:{}:{}", auth_context.session_id, text);
            if let Some(cached) = self.get_cached_response(&cache_key).await {
                debug!("Returning cached AI response for text: {}", text);
                return Ok(cached);
            }
        }

        // Get session state for context
        let session = self
            .session_manager
            .get_session(&auth_context.session_id)
            .await?;
        let session_state = session.read().await;

        // Build rich context
        let context = ConversationContext {
            session_id: auth_context.session_id.clone(),
            previous_commands: vec![], // Skip previous commands for now
            active_objects: session_state
                .objects
                .keys()
                .map(|id| id.to_string())
                .collect(),
            user_preferences: self.get_user_preferences(&auth_context.user_id).await,
            scene_state: None, // Skip scene state for now
            system_context: Some(shared_types::SystemContext::default()),
        };

        // Step 2: Parse with LLM
        let parsed_command = {
            let manager = self.provider_manager.lock().await;
            let llm = manager.llm()?;
            llm.process(text, Some(&context)).await?
        };

        // Step 3: Check permissions
        if self.config.enforce_permissions {
            self.check_command_permissions(&auth_context, &parsed_command)?;
        }

        // Step 4: Execute command
        let result = self
            .execute_with_session(&auth_context, &parsed_command)
            .await?;

        let processed = ProcessedCommand {
            original_text: text.to_string(),
            command: parsed_command,
            result: result.clone(),
            execution_time_ms: start.elapsed().as_millis() as u64,
        };

        // Cache the result
        if self.config.enable_caching {
            let cache_key = format!("text:{}:{}", auth_context.session_id, text);
            self.cache_response(&cache_key, &processed).await;
        }

        // Broadcast to session if enabled
        if self.config.enable_broadcasting {
            self.broadcast_ai_activity(&auth_context, &processed)
                .await?;
        }

        // Audit log
        if self.config.enable_audit_log {
            self.log_ai_operation(&auth_context, &processed).await;
        }

        Ok(processed)
    }

    /// Check if user has permission for the command
    fn check_command_permissions(
        &self,
        auth_context: &AIAuthContext,
        command: &ParsedCommand,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        use Permission::*;

        let required_permission = match &command.intent {
            CommandIntent::CreatePrimitive { .. } => CreateGeometry,
            CommandIntent::Transform { .. } => ModifyGeometry,
            CommandIntent::BooleanOperation { .. } => BooleanOperations,
            CommandIntent::Query { .. } => ViewGeometry,
            CommandIntent::Extrude { .. } => ModifyGeometry,
            CommandIntent::Create { .. } => CreateGeometry,
            CommandIntent::Modify { .. } => ModifyGeometry,
            CommandIntent::Boolean { .. } => BooleanOperations,
            CommandIntent::Export { .. } => ViewGeometry,
            CommandIntent::Import { .. } => CreateGeometry,
            CommandIntent::Unknown => ViewGeometry,
        };

        // Check if user has the required permission
        let has_permission = auth_context
            .permissions
            .explicit_permissions
            .contains(&required_permission)
            || (self
                .get_role_permissions(&auth_context.permissions.role)
                .contains(&required_permission)
                && !auth_context
                    .permissions
                    .denied_permissions
                    .contains(&required_permission));

        if !has_permission {
            return Err(format!(
                "Permission denied: {} required for this operation",
                format!("{:?}", required_permission)
            )
            .into());
        }

        Ok(())
    }

    /// Execute command with session integration
    async fn execute_with_session(
        &self,
        auth_context: &AIAuthContext,
        command: &ParsedCommand,
    ) -> Result<CommandResult, Box<dyn std::error::Error + Send + Sync>> {
        // Convert to AICommand for session manager
        let ai_command = self.convert_to_ai_command(command)?;

        // Execute through session manager for proper tracking
        let result = self
            .session_manager
            .process_command(&auth_context.session_id, ai_command, &auth_context.user_id)
            .await?;

        // Convert result - use the actual CommandResult structure from shared_types
        if result.success {
            // If there are objects affected, create success with primary object
            if !result.objects_affected.is_empty() {
                let object_ids = result
                    .objects_affected
                    .iter()
                    .map(|id| GeometryId(*id))
                    .collect::<Vec<_>>();
                Ok(create_success_result_with_objects(
                    "Command executed successfully",
                    object_ids,
                ))
            } else if let Some(data) = result.data {
                // If there's data, it's a query result
                Ok(create_query_result("Query executed successfully", data))
            } else {
                // Default success result
                Ok(create_success_result_with_objects(
                    "Command executed successfully",
                    vec![],
                ))
            }
        } else {
            Ok(create_error_result(result.message))
        }
    }

    /// Get cached response
    async fn get_cached_response(&self, cache_key: &str) -> Option<ProcessedCommand> {
        let cache_manager = self.session_manager.cache_manager();

        cache_manager
            .get_computed_geometry(cache_key)
            .and_then(|json| serde_json::from_value(json).ok())
    }

    /// Cache response
    async fn cache_response(&self, cache_key: &str, response: &ProcessedCommand) {
        let cache_manager = self.session_manager.cache_manager();

        if let Ok(json) = serde_json::to_value(response) {
            cache_manager.cache_computed_geometry(
                cache_key.to_string(),
                json,
                std::time::Duration::from_secs(self.config.cache_ttl_seconds),
            );
        }
    }

    /// Compute cache key for audio
    fn compute_audio_cache_key(&self, audio: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(audio);
        format!("audio:{:x}", hasher.finalize())
    }

    /// Get recent commands for context
    async fn get_recent_commands(&self, session_id: &str) -> Vec<String> {
        // In real implementation, query from timeline
        vec![]
    }

    /// Get user preferences
    async fn get_user_preferences(&self, user_id: &str) -> serde_json::Value {
        // In real implementation, load from database
        serde_json::json!({
            "default_units": "mm",
            "preferred_language": "en",
            "ai_assistance_level": "intermediate"
        })
    }

    /// Build scene state from session
    fn build_scene_state(&self, session: &shared_types::SessionState) -> serde_json::Value {
        serde_json::json!({
            "object_count": session.objects.len(),
            "object_types": self.get_object_types(&session.objects),
            "bounds": self.calculate_scene_bounds(&session.objects),
            "active_users": session.active_users.len(),
        })
    }

    /// Get object types in scene
    fn get_object_types(
        &self,
        objects: &HashMap<ObjectId, shared_types::CADObject>,
    ) -> HashMap<String, usize> {
        let mut types = HashMap::new();
        for _obj in objects.values() {
            *types.entry("CADObject".to_string()).or_insert(0) += 1;
        }
        types
    }

    /// Calculate scene bounds
    fn calculate_scene_bounds(
        &self,
        objects: &HashMap<ObjectId, shared_types::CADObject>,
    ) -> [f64; 6] {
        // Simplified - in real implementation, calculate actual bounds
        [-100.0, -100.0, -100.0, 100.0, 100.0, 100.0]
    }

    /// Convert parsed command to AICommand type
    fn convert_to_ai_command(
        &self,
        parsed: &ParsedCommand,
    ) -> Result<shared_types::AICommand, Box<dyn std::error::Error + Send + Sync>> {
        use shared_types::{AICommand, PrimitiveType, ShapeParameters};

        match &parsed.intent {
            CommandIntent::CreatePrimitive { shape } => {
                // Extract parameters from parsed command
                let params = &parsed.parameters;

                match shape.as_str() {
                    "box" => {
                        let width = params.get("width").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let depth = params.get("depth").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        Ok(AICommand::CreatePrimitive {
                            shape_type: PrimitiveType::Box,
                            parameters: ShapeParameters::box_params(width, height, depth),
                            position: [0.0, 0.0, 0.0],
                            material: None,
                        })
                    }
                    "sphere" => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        Ok(AICommand::CreatePrimitive {
                            shape_type: PrimitiveType::Sphere,
                            parameters: ShapeParameters::sphere_params(radius),
                            position: [0.0, 0.0, 0.0],
                            material: None,
                        })
                    }
                    "cylinder" => {
                        let radius = params.get("radius").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        let height = params.get("height").and_then(|v| v.as_f64()).unwrap_or(2.0);
                        Ok(AICommand::CreatePrimitive {
                            shape_type: PrimitiveType::Cylinder,
                            parameters: ShapeParameters::cylinder_params(radius, height),
                            position: [0.0, 0.0, 0.0],
                            material: None,
                        })
                    }
                    _ => Err("Unsupported shape type".into()),
                }
            }
            CommandIntent::Transform { operation: _ } => {
                // For now, return a simple transform
                Ok(AICommand::Transform {
                    object_id: shared_types::ObjectId::new_v4(),
                    transform_type: shared_types::TransformType::Translate {
                        offset: [1.0, 0.0, 0.0],
                    },
                })
            }
            _ => Err("Unsupported command type".into()),
        }
    }

    /// Parse primitive parameters into JSON
    fn parse_primitive_params(
        &self,
        object_type: &str,
        params: &serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error + Send + Sync>> {
        match object_type {
            "box" | "cube" => Ok(serde_json::json!({
                "width": params["width"].as_f64().unwrap_or(1.0),
                "height": params["height"].as_f64().unwrap_or(1.0),
                "depth": params["depth"].as_f64().unwrap_or(1.0),
            })),
            "sphere" => Ok(serde_json::json!({
                "radius": params["radius"].as_f64().unwrap_or(1.0),
                "u_segments": 32,
                "v_segments": 16,
            })),
            "cylinder" => Ok(serde_json::json!({
                "radius": params["radius"].as_f64().unwrap_or(1.0),
                "height": params["height"].as_f64().unwrap_or(2.0),
                "segments": 32,
            })),
            _ => Err(format!("Unknown primitive type: {}", object_type).into()),
        }
    }

    /// Broadcast AI activity to session
    async fn broadcast_ai_activity(
        &self,
        auth_context: &AIAuthContext,
        command: &ProcessedCommand,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let broadcast_manager = self.session_manager.broadcast_manager();

        // Create appropriate broadcast message based on command result
        let message = if command.result.success {
            if let Some(object_id) = command.result.object_id {
                // Create a simple object created message
                let dummy_object = shared_types::CADObject::new_mesh_object(
                    object_id.0,
                    "AI Created Object".to_string(),
                    shared_types::Mesh::new(),
                );
                session_manager::BroadcastMessage::ObjectCreated {
                    session_id: Uuid::parse_str(&auth_context.session_id)?,
                    object: dummy_object,
                    created_by: auth_context.user_id.clone(),
                }
            } else {
                // Use a cursor moved message for general success (simple message)
                session_manager::BroadcastMessage::CursorMoved {
                    session_id: Uuid::parse_str(&auth_context.session_id)?,
                    user_id: auth_context.user_id.clone(),
                    position: [0.0, 0.0, 0.0],
                }
            }
        } else {
            // Use a cursor moved message as a fallback for errors
            session_manager::BroadcastMessage::CursorMoved {
                session_id: Uuid::parse_str(&auth_context.session_id)?,
                user_id: auth_context.user_id.clone(),
                position: [0.0, 0.0, 0.0],
            }
        };

        broadcast_manager
            .broadcast_to_session(&auth_context.session_id, message)
            .await
            .map(|_| ())
            .map_err(|e| e.into())
    }

    /// Log AI operation for audit
    async fn log_ai_operation(&self, auth_context: &AIAuthContext, command: &ProcessedCommand) {
        info!(
            "AI Operation - User: {}, Session: {}, Command: {}, Result: {:?}, Time: {}ms",
            auth_context.user_id,
            auth_context.session_id,
            command.original_text,
            command.result,
            command.execution_time_ms
        );
    }

    /// Clear authentication context (on logout)
    pub async fn clear_auth_context(&self, token: &str) {
        let mut contexts = self.auth_contexts.write().await;
        contexts.remove(token);
    }

    /// Get active sessions for a user
    pub async fn get_user_sessions(&self, user_id: &str) -> Vec<String> {
        let contexts = self.auth_contexts.read().await;
        contexts
            .values()
            .filter(|ctx| ctx.user_id == user_id)
            .map(|ctx| ctx.session_id.clone())
            .collect()
    }
}

// Removed From<Permission> for String implementation to avoid orphan rule violation

impl SessionAwareAIProcessor {
    fn get_role_permissions(&self, role: &session_manager::Role) -> HashSet<Permission> {
        use session_manager::Role;
        use Permission::*;
        let mut permissions = HashSet::new();

        match role {
            Role::Owner => {
                // Owner has all permissions
                permissions.insert(CreateGeometry);
                permissions.insert(ModifyGeometry);
                permissions.insert(DeleteGeometry);
                permissions.insert(ViewGeometry);
                permissions.insert(ExportGeometry);
                permissions.insert(BooleanOperations);
                permissions.insert(UndoRedo);
            }
            Role::Editor => {
                // Editor can create and modify
                permissions.insert(CreateGeometry);
                permissions.insert(ModifyGeometry);
                permissions.insert(ViewGeometry);
                permissions.insert(BooleanOperations);
                permissions.insert(UndoRedo);
            }
            Role::Viewer => {
                // Viewer can only view
                permissions.insert(ViewGeometry);
            }
            _ => {}
        }

        permissions
    }
}

// Removed Permission::from_string implementation - define a local helper function instead
fn permission_from_string(s: &str) -> Option<Permission> {
    match s {
        "create_objects" => Some(Permission::CreateGeometry),
        "edit_objects" => Some(Permission::ModifyGeometry),
        "delete_objects" => Some(Permission::DeleteGeometry),
        "view_objects" => Some(Permission::ViewGeometry),
        "export_session" => Some(Permission::ExportGeometry),
        "import_files" => Some(Permission::CreateGeometry),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_aware_processor() {
        // Test implementation
    }
}
