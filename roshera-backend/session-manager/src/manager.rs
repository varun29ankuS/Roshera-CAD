//! Session manager implementation
//!
//! Handles creation, retrieval, and management of CAD sessions.

use crate::broadcast::{BroadcastManager, BroadcastMessage};
use crate::command_processor::CommandProcessor;
use crate::conflict_resolution::{GeometryCRDT, OTEngine, Operation, TransformedOperation};
use crate::state::{SessionStateExt, SharedSessionState};
use dashmap::DashMap;
use shared_types::session::UserInfo;
use shared_types::*;
use std::collections::VecDeque;
use std::sync::Arc;
use timeline_engine::{Timeline, TimelineConfig};
use tokio::sync::RwLock;
use tracing::{info, warn};
use uuid::Uuid;

/// Manages multiple CAD sessions with timeline support
#[derive(Clone)]
pub struct SessionManager {
    /// Active sessions using DashMap for concurrent access
    sessions: Arc<DashMap<String, SharedSessionState>>,
    /// Broadcast manager for real-time updates
    broadcast_manager: BroadcastManager,
    /// Command processor with timeline integration
    command_processor: Arc<CommandProcessor>,
    /// Timeline engine for history tracking
    timeline: Arc<RwLock<Timeline>>,
    /// Operational Transformation engine for conflict resolution
    ot_engine: Arc<OTEngine>,
    /// CRDT state for each session
    session_crdts: Arc<DashMap<String, GeometryCRDT>>,
}

impl SessionManager {
    /// Creates a new session manager with timeline support
    pub fn new(broadcast_manager: BroadcastManager) -> Self {
        let timeline_config = TimelineConfig::default();
        let timeline = Arc::new(RwLock::new(Timeline::new(timeline_config)));
        let command_processor = Arc::new(CommandProcessor::new(timeline.clone()));
        let ot_engine = Arc::new(OTEngine::new());

        Self {
            sessions: Arc::new(DashMap::new()),
            broadcast_manager,
            command_processor,
            timeline,
            ot_engine,
            session_crdts: Arc::new(DashMap::new()),
        }
    }

    /// Get auth manager for authentication operations
    pub fn auth_manager(&self) -> &crate::auth::AuthManager {
        // For now, return a default auth manager
        // In a real implementation, this would be a field in SessionManager
        static AUTH_MANAGER: std::sync::OnceLock<crate::auth::AuthManager> =
            std::sync::OnceLock::new();
        AUTH_MANAGER.get_or_init(|| {
            crate::auth::AuthManager::new(crate::auth::AuthConfig::default(), "default-jwt-secret")
        })
    }

    /// Get permission manager for permission operations
    pub fn permission_manager(&self) -> &crate::permissions::PermissionManager {
        // For now, return a default permission manager
        // In a real implementation, this would be a field in SessionManager
        static PERMISSION_MANAGER: std::sync::OnceLock<crate::permissions::PermissionManager> =
            std::sync::OnceLock::new();
        PERMISSION_MANAGER.get_or_init(|| crate::permissions::PermissionManager::new())
    }

    /// Get cache manager for caching operations
    pub fn cache_manager(&self) -> &crate::cache::CacheManager {
        // For now, return a default cache manager
        // In a real implementation, this would be a field in SessionManager
        static CACHE_MANAGER: std::sync::OnceLock<crate::cache::CacheManager> =
            std::sync::OnceLock::new();
        CACHE_MANAGER
            .get_or_init(|| crate::cache::CacheManager::new(crate::cache::CacheConfig::default()))
    }

    /// Get delta manager for delta synchronization operations
    pub fn delta_manager(&self) -> &crate::delta_manager::DeltaManager {
        // For now, return a default delta manager
        // In a real implementation, this would be a field in SessionManager
        static DELTA_MANAGER: std::sync::OnceLock<crate::delta_manager::DeltaManager> =
            std::sync::OnceLock::new();
        DELTA_MANAGER.get_or_init(|| crate::delta_manager::DeltaManager::new())
    }

    /// Get broadcast manager for real-time updates
    pub fn broadcast_manager(&self) -> &BroadcastManager {
        &self.broadcast_manager
    }

    /// Creates a new session
    pub async fn create_session(&self, user_name: String) -> String {
        let session_id = Uuid::new_v4();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_millis() as u64;

        let session = SessionState::new(session_id, user_name.clone());

        let session_ref = Arc::new(RwLock::new(session));

        // Using DashMap's insert method directly
        self.sessions
            .insert(session_id.to_string(), session_ref.clone());

        // Broadcast session creation
        let user_info = UserInfo {
            id: user_name.clone(),
            name: user_name,
            color: [0.0, 0.5, 1.0, 1.0], // Default blue color
            last_activity: now,
            role: shared_types::session::UserRole::Owner,
            cursor_position: None,
            selected_objects: Vec::new(),
        };
        let message = BroadcastMessage::UserJoined {
            session_id,
            user: user_info,
        };
        let _ = self
            .broadcast_manager
            .broadcast_to_session(&session_id.to_string(), message)
            .await;

        session_id.to_string()
    }

    /// Gets a session by ID
    pub async fn get_session(&self, session_id: &str) -> Result<SharedSessionState, SessionError> {
        self.sessions
            .get(session_id)
            .map(|entry| entry.value().clone())
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.to_string(),
            })
    }

    /// Adds an object to a session
    pub async fn add_object(
        &self,
        session_id: &str,
        object: CADObject,
    ) -> Result<(), SessionError> {
        let session = self.get_session(session_id).await?;

        // Capture old state for delta
        let old_state = {
            let state = session.read().await;
            state.clone()
        };

        // Add object
        let mut state = session.write().await;
        state.objects.insert(object.id, object.clone());
        state.update_modified_timestamp();

        // Create new state snapshot
        let new_state = state.clone();

        // Release lock before broadcasting
        drop(state);

        // Broadcast delta update
        self.broadcast_manager
            .broadcast_delta(session_id, &old_state, &new_state)
            .await?;

        Ok(())
    }

    /// Lists all session IDs
    pub async fn list_session_ids(&self) -> Vec<String> {
        self.sessions
            .iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Performs undo operation using timeline engine
    pub async fn undo_operation(&self, session_id: &str) -> Result<CommandResult, SessionError> {
        info!("Performing undo for session {}", session_id);

        // Use the command processor which integrates with timeline
        self.command_processor
            .process_command(
                session_id,
                AICommand::SessionControl {
                    action: SessionAction::Undo,
                },
                "system",
            )
            .await
    }

    /// Performs redo operation using timeline engine
    pub async fn redo_operation(&self, session_id: &str) -> Result<CommandResult, SessionError> {
        info!("Performing redo for session {}", session_id);

        // Use the command processor which integrates with timeline
        self.command_processor
            .process_command(
                session_id,
                AICommand::SessionControl {
                    action: SessionAction::Redo,
                },
                "system",
            )
            .await
    }

    /// Remove an object from a session
    pub async fn remove_object(
        &self,
        session_id: &str,
        object_id: &str,
    ) -> Result<(), SessionError> {
        let session = self.get_session(session_id).await?;

        // Capture old state for delta
        let old_state = {
            let state = session.read().await;
            state.clone()
        };

        let mut state = session.write().await;

        // Parse object_id to ObjectId (Uuid)
        let object_id_parsed = Uuid::parse_str(object_id).map_err(|_| SessionError::NotFound {
            id: object_id.to_string(),
        })?;

        // Remove the object
        state
            .objects
            .remove(&object_id_parsed)
            .ok_or_else(|| SessionError::NotFound {
                id: object_id.to_string(),
            })?;

        state.update_modified_timestamp();

        // Create new state snapshot
        let new_state = state.clone();

        // Release lock before broadcasting
        drop(state);

        // Broadcast delta update
        self.broadcast_manager
            .broadcast_delta(session_id, &old_state, &new_state)
            .await?;

        Ok(())
    }

    /// List all active sessions
    pub async fn list_sessions(&self) -> Vec<serde_json::Value> {
        let mut sessions = Vec::new();

        for entry in self.sessions.iter() {
            let session_id = entry.key();
            let session_state = entry.value();
            let state = session_state.read().await;

            sessions.push(serde_json::json!({
                "id": session_id,
                "name": state.name,
                "created_at": state.created_at,
                "modified_at": state.modified_at,
                "active_users": state.active_users.len(),
                "object_count": state.objects.len(),
            }));
        }

        sessions
    }

    /// Delete a session
    pub async fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        info!("Deleting session {}", session_id);

        // Remove from sessions map
        self.sessions
            .remove(session_id)
            .ok_or_else(|| SessionError::NotFound {
                id: session_id.to_string(),
            })?;

        // Remove from CRDTs
        self.session_crdts.remove(session_id);

        // Broadcast session deletion
        self.broadcast_manager
            .broadcast(BroadcastMessage::SessionDeleted {
                session_id: session_id.to_string(),
            })
            .await;

        Ok(())
    }

    /// Join a session
    pub async fn join_session(&self, session_id: &str, user_id: &str) -> Result<(), SessionError> {
        info!("User {} joining session {}", user_id, session_id);

        let session = self.get_session(session_id).await?;
        let mut state = session.write().await;

        // Add user to active users
        let user_info = UserInfo::new(user_id.to_string(), user_id.to_string());
        if !state.active_users.iter().any(|u| u.id == user_id) {
            state.active_users.push(user_info);
        }

        state.update_modified_timestamp();

        // Broadcast user joined
        let session_uuid = Uuid::parse_str(session_id).unwrap_or_else(|_| Uuid::new_v4());
        let user_info = UserInfo::new(user_id.to_string(), user_id.to_string());
        self.broadcast_manager
            .broadcast(BroadcastMessage::UserJoined {
                session_id: session_uuid,
                user: user_info,
            })
            .await;

        Ok(())
    }

    /// Leave a session
    pub async fn leave_session(&self, session_id: &str, user_id: &str) -> Result<(), SessionError> {
        info!("User {} leaving session {}", user_id, session_id);

        let session = self.get_session(session_id).await?;
        let mut state = session.write().await;

        // Remove user from active users
        state.active_users.retain(|u| u.id != user_id);
        state.update_modified_timestamp();

        // Broadcast user left
        let session_uuid = Uuid::parse_str(session_id).unwrap_or_else(|_| Uuid::new_v4());
        self.broadcast_manager
            .broadcast(BroadcastMessage::UserLeft {
                session_id: session_uuid,
                user_id: user_id.to_string(),
            })
            .await;

        Ok(())
    }

    /// Process any geometry command with timeline tracking
    pub async fn process_command(
        &self,
        session_id: &str,
        command: AICommand,
        user_id: &str,
    ) -> Result<CommandResult, SessionError> {
        info!(
            "Processing command for session {}: {:?}",
            session_id, command
        );

        // Verify session exists and capture old state
        let session = self.get_session(session_id).await?;

        let old_state = {
            let state = session.read().await;
            state.clone()
        };

        // Process through command processor (handles timeline)
        let result = self
            .command_processor
            .process_command(session_id, command, user_id)
            .await?;

        // Broadcast the result if successful
        if result.success {
            // Get new state after command execution
            let new_state = {
                let state = session.read().await;
                state.clone()
            };

            // Broadcast delta update
            self.broadcast_manager
                .broadcast_delta(session_id, &old_state, &new_state)
                .await?;
        }

        Ok(result)
    }

    /// Get command processor for direct access
    pub fn command_processor(&self) -> &Arc<CommandProcessor> {
        &self.command_processor
    }

    /// Get timeline for direct access
    pub fn timeline(&self) -> &Arc<RwLock<Timeline>> {
        &self.timeline
    }

    /// Process command with conflict resolution using OT
    pub async fn process_command_with_ot(
        &self,
        session_id: &str,
        command: AICommand,
        user_id: &str,
    ) -> Result<CommandResult, SessionError> {
        info!(
            "Processing command with OT for session {}: {:?}",
            session_id, command
        );

        // Get session and capture old state
        let session = self.get_session(session_id).await?;

        let old_state = {
            let state = session.read().await;
            state.clone()
        };

        // Create operation from command
        let operation = Operation {
            id: Uuid::new_v4(),
            command: command.clone(),
            timestamp: current_timestamp(),
            user_id: user_id.to_string(),
            dependencies: vec![],
        };

        // Apply through OT engine
        let operations = self
            .ot_engine
            .apply_operations(session_id, vec![operation])
            .await
            .map_err(|e| SessionError::ConflictError { details: e })?;

        // Process the transformed operation
        if let Some(transformed_op) = operations.first() {
            let result = self
                .command_processor
                .process_command(session_id, transformed_op.command.clone(), user_id)
                .await?;

            // Update CRDT state if successful
            if result.success {
                self.update_crdt_state(session_id, &transformed_op.command)
                    .await;

                // Get new state and broadcast delta
                let new_state = {
                    let state = session.read().await;
                    state.clone()
                };

                self.broadcast_manager
                    .broadcast_delta(session_id, &old_state, &new_state)
                    .await?;
            }

            Ok(result)
        } else {
            Err(SessionError::ConflictError {
                details: "Operation was rejected by OT engine".to_string(),
            })
        }
    }

    /// Update CRDT state for a session
    async fn update_crdt_state(&self, session_id: &str, command: &AICommand) {
        let crdt = self
            .session_crdts
            .entry(session_id.to_string())
            .or_insert_with(|| GeometryCRDT::new(session_id.to_string()));

        // Update CRDT based on command type
        match command {
            AICommand::Transform {
                object_id,
                transform_type,
            } => {
                let transform_json = serde_json::to_value(transform_type).unwrap_or_default();
                crdt.update_property(
                    *object_id,
                    "transform".to_string(),
                    transform_json,
                    current_timestamp(),
                );
            }
            AICommand::ModifyMaterial {
                object_id,
                material,
            } => {
                crdt.update_property(
                    *object_id,
                    "material".to_string(),
                    serde_json::json!(material),
                    current_timestamp(),
                );
            }
            _ => {
                // Other commands don't need CRDT updates
            }
        }
    }

    /// Merge CRDT states from another replica
    pub async fn merge_crdt_state(&self, session_id: &str, other_crdt: &GeometryCRDT) {
        if let Some(crdt) = self.session_crdts.get(&session_id.to_string()) {
            crdt.merge(other_crdt);
        }
    }

    /// Send initial snapshot to new client
    pub async fn send_snapshot_to_client(
        &self,
        session_id: &str,
        client_id: &str,
    ) -> Result<(), SessionError> {
        let session = self.get_session(session_id).await?;
        let state = session.read().await;

        // Create snapshot delta for new client
        let snapshot_delta = self
            .broadcast_manager
            .create_snapshot(session_id, &*state)
            .await?;

        // Send snapshot as compressed delta
        if let Ok(compressed) = crate::delta::compression::compress_delta(&snapshot_delta) {
            let message = BroadcastMessage::CompressedDeltaUpdate {
                session_id: Uuid::parse_str(session_id)?,
                data: compressed,
                original_size: serde_json::to_vec(&snapshot_delta)?.len(),
            };

            // In real implementation, this would send to specific client
            // For now, broadcast to session (client would filter by ID)
            self.broadcast_manager
                .broadcast_to_session(session_id, message)
                .await
                .map_err(|_| SessionError::PersistenceError {
                    reason: "Failed to send snapshot".to_string(),
                })?;
        }

        Ok(())
    }

    /// Get the OT engine for advanced conflict resolution
    pub fn ot_engine(&self) -> &Arc<OTEngine> {
        &self.ot_engine
    }
}

/// Get current timestamp in milliseconds
fn current_timestamp() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_millis() as u64
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(BroadcastManager::new())
    }
}
