//! Session manager implementation
//!
//! Handles creation, retrieval, and management of CAD sessions.

use crate::broadcast::{BroadcastManager, BroadcastMessage};
use crate::command_processor::CommandProcessor;
use crate::conflict_resolution::{GeometryCRDT, OTEngine, Operation};
use crate::state::{SessionStateExt, SharedSessionState};
use dashmap::DashMap;
use shared_types::session::UserInfo;
use shared_types::*;
use std::sync::Arc;
use timeline_engine::{Timeline, TimelineConfig};
use tokio::sync::RwLock;
use tracing::info;
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
    /// Authentication manager for JWT and API key handling
    auth_manager: Arc<crate::auth::AuthManager>,
    /// Permission manager for RBAC enforcement
    permission_manager: Arc<crate::permissions::PermissionManager>,
    /// Cache manager for multi-layer caching
    cache_manager: Arc<crate::cache::CacheManager>,
    /// Delta manager for delta synchronization
    delta_manager: Arc<crate::delta_manager::DeltaManager>,
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
            auth_manager: build_auth_manager(),
            permission_manager: Arc::new(crate::permissions::PermissionManager::new()),
            cache_manager: Arc::new(crate::cache::CacheManager::new(
                crate::cache::CacheConfig::default(),
            )),
            delta_manager: Arc::new(crate::delta_manager::DeltaManager::new()),
        }
    }

    /// Get auth manager for authentication operations
    pub fn auth_manager(&self) -> &crate::auth::AuthManager {
        &self.auth_manager
    }

    /// Get permission manager for permission operations
    pub fn permission_manager(&self) -> &crate::permissions::PermissionManager {
        &self.permission_manager
    }

    /// Get cache manager for caching operations
    pub fn cache_manager(&self) -> &crate::cache::CacheManager {
        &self.cache_manager
    }

    /// Get delta manager for delta synchronization operations
    pub fn delta_manager(&self) -> &crate::delta_manager::DeltaManager {
        &self.delta_manager
    }

    /// Get broadcast manager for real-time updates
    pub fn broadcast_manager(&self) -> &BroadcastManager {
        &self.broadcast_manager
    }

    /// Creates a new session
    pub async fn create_session(&self, user_name: String) -> String {
        let session_id = Uuid::new_v4();
        let now = shared_types::unix_millis_now();

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
        _client_id: &str,
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
    shared_types::unix_millis_now()
}

/// Build the `Arc<AuthManager>` used by [`SessionManager::new`].
///
/// Separated into a free function so the `#[allow(clippy::expect_used)]`
/// invariant comment lives at statement scope (Rust does not allow
/// inner attributes on method-call expressions).
fn build_auth_manager() -> Arc<crate::auth::AuthManager> {
    let secret = load_jwt_secret();
    // `load_jwt_secret` returns either the operator-supplied
    // `ROSHERA_JWT_SECRET` (guaranteed non-empty by the selector) or a
    // freshly-sampled 32-byte random hex string. Both branches yield a
    // non-empty string, which is the only constraint HMAC-SHA256
    // imposes; `AuthManager::new` can only fail on the empty case it
    // explicitly screens out.
    #[allow(clippy::expect_used)]
    // Reason: invariant proved above — secret is non-empty by construction.
    let mgr = crate::auth::AuthManager::new(crate::auth::AuthConfig::default(), &secret)
        .expect("load_jwt_secret returns a non-empty HMAC key");
    Arc::new(mgr)
}

/// Resolve the JWT signing secret.
///
/// Precedence:
/// 1. `ROSHERA_JWT_SECRET` environment variable, if set and non-empty.
///    This is the only way to produce a secret that survives a process
///    restart or is shared across replicas — production deployments
///    MUST set it.
/// 2. A per-process random 32-byte secret (rendered as 64 hex chars),
///    sampled from `OsRng` (the OS CSPRNG). This branch logs a
///    `tracing::warn!` so the operator notices when running without
///    a configured secret. JWTs issued under this fallback are
///    invalidated on every restart and cannot be validated by any
///    other process, which is the desired failure mode for local dev:
///    insecure config does not silently leak into production.
///
/// The previous hard-coded literal (`"default-jwt-secret"`) allowed
/// anyone with source-code access to forge valid JWTs against any
/// running instance — an unrestricted privilege-escalation primitive.
/// This function removes that primitive while keeping the dev
/// workflow ergonomic.
fn load_jwt_secret() -> String {
    if let Ok(env_secret) = std::env::var("ROSHERA_JWT_SECRET") {
        if !env_secret.is_empty() {
            return env_secret;
        }
    }
    let mut bytes = [0u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut bytes);
    let hex: String = bytes.iter().map(|b| format!("{:02x}", b)).collect();
    tracing::warn!(
        target: "session_manager.auth",
        "ROSHERA_JWT_SECRET is unset or empty — generated an ephemeral \
         per-process secret. Issued JWTs will be invalidated on restart \
         and cannot be shared across replicas. Set ROSHERA_JWT_SECRET in \
         production deployments."
    );
    hex
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new(BroadcastManager::new())
    }
}
