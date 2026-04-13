//! Broadcasting system for real-time updates
//!
//! Handles WebSocket message broadcasting to connected clients.

use crate::delta::{compression, DeltaTracker, SessionDelta};
use serde::{Deserialize, Serialize};
use shared_types::session::UserInfo;
use shared_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};
use uuid::Uuid;

/// Message types for WebSocket communication
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BroadcastMessage {
    /// Object was created
    ObjectCreated {
        session_id: Uuid,
        object: CADObject,
        created_by: String,
    },

    /// Object was modified
    ObjectModified {
        session_id: Uuid,
        object: CADObject,
        modified_by: String,
    },

    /// Object was deleted
    ObjectDeleted {
        session_id: Uuid,
        object_id: ObjectId,
        deleted_by: String,
    },

    /// User joined session
    UserJoined { session_id: Uuid, user: UserInfo },

    /// User left session
    UserLeft { session_id: Uuid, user_id: String },

    /// Session state update
    StateUpdate {
        session_id: Uuid,
        state: SessionStateSnapshot,
    },

    /// Cursor moved
    CursorMoved {
        session_id: Uuid,
        user_id: String,
        position: Position3D,
    },

    /// Command executed
    CommandExecuted {
        session_id: Uuid,
        command: AICommand,
        result: CommandResult,
        executed_by: String,
    },

    /// Delta update (efficient state synchronization)
    DeltaUpdate {
        session_id: Uuid,
        delta: SessionDelta,
        compressed: bool,
    },

    /// Compressed delta update (for network efficiency)
    CompressedDeltaUpdate {
        session_id: Uuid,
        data: Vec<u8>,
        original_size: usize,
    },

    /// Timeline update
    TimelineUpdate {
        session_id: Uuid,
        event_id: String,
        operation: String,
        user_id: String,
    },

    /// Session was deleted
    SessionDeleted { session_id: String },
}

// Add convenience constructors
impl BroadcastMessage {
    /// Create user joined message
    pub fn user_joined(session_id: Uuid, user: UserInfo) -> Self {
        Self::UserJoined { session_id, user }
    }

    /// Create user left message
    pub fn user_left(session_id: Uuid, user_id: String) -> Self {
        Self::UserLeft {
            session_id,
            user_id,
        }
    }

    /// Create object created message
    pub fn object_created(session_id: Uuid, object: CADObject, created_by: String) -> Self {
        Self::ObjectCreated {
            session_id,
            object,
            created_by,
        }
    }
}

/// Configuration for broadcast manager
#[derive(Clone)]
pub struct BroadcastConfig {
    /// Channel capacity
    pub capacity: usize,
    /// Enable message compression
    pub compress_messages: bool,
    /// Maximum message size in bytes
    pub max_message_size: usize,
    /// Message retention time in seconds
    pub retention_seconds: u64,
    /// Enable delta updates instead of full state
    pub use_delta_updates: bool,
    /// Batch delta updates for efficiency
    pub batch_deltas: bool,
    /// Delta batch window in milliseconds
    pub delta_batch_window_ms: u64,
}

impl Default for BroadcastConfig {
    fn default() -> Self {
        Self {
            capacity: 1000,
            compress_messages: false,
            max_message_size: 1024 * 1024, // 1MB
            retention_seconds: 3600,       // 1 hour
            use_delta_updates: true,
            batch_deltas: true,
            delta_batch_window_ms: 50, // 50ms batching window
        }
    }
}

/// Snapshot of session state for broadcasting
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStateSnapshot {
    pub objects: Vec<CADObject>,
    pub active_users: Vec<UserInfo>,
    pub timestamp: u64,
}

/// Manages broadcasting to connected clients
#[derive(Clone)]
pub struct BroadcastManager {
    /// Broadcast channels per session
    channels: Arc<RwLock<HashMap<String, broadcast::Sender<BroadcastMessage>>>>,
    /// Configuration
    config: BroadcastConfig,
    /// Delta trackers per session
    delta_trackers: Arc<RwLock<HashMap<String, DeltaTracker>>>,
    /// Pending deltas for batching
    pending_deltas: Arc<RwLock<HashMap<String, Vec<SessionDelta>>>>,
}

impl BroadcastManager {
    /// Creates a new broadcast manager
    pub fn new() -> Self {
        Self::with_config(BroadcastConfig::default())
    }

    /// Creates a new broadcast manager with config
    pub fn with_config(config: BroadcastConfig) -> Self {
        Self {
            channels: Arc::new(RwLock::new(HashMap::new())),
            config,
            delta_trackers: Arc::new(RwLock::new(HashMap::new())),
            pending_deltas: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Create a channel for a session
    pub fn create_session_channel(
        &self,
        session_id: ObjectId,
    ) -> broadcast::Receiver<BroadcastMessage> {
        let mut channels = self.channels.blocking_write();
        let (tx, rx) = broadcast::channel(self.config.capacity);
        channels.insert(session_id.to_string(), tx);
        rx
    }

    /// Broadcasts a message to all clients in a session
    pub async fn broadcast_to_session(
        &self,
        session_id: &str,
        message: BroadcastMessage,
    ) -> Result<usize, broadcast::error::SendError<BroadcastMessage>> {
        let channels = self.channels.read().await;
        if let Some(sender) = channels.get(session_id) {
            sender.send(message)
        } else {
            Ok(0) // No subscribers
        }
    }

    /// Broadcast to all sessions
    pub async fn broadcast(
        &self,
        message: BroadcastMessage,
    ) -> Result<usize, broadcast::error::SendError<BroadcastMessage>> {
        let channels = self.channels.read().await;
        let mut total = 0;
        for sender in channels.values() {
            if let Ok(count) = sender.send(message.clone()) {
                total += count;
            }
        }
        Ok(total)
    }

    /// Broadcast delta update instead of full state
    pub async fn broadcast_delta(
        &self,
        session_id: &str,
        old_state: &SessionState,
        new_state: &SessionState,
    ) -> Result<usize, SessionError> {
        if !self.config.use_delta_updates {
            // Fall back to full state update
            let snapshot = SessionStateSnapshot {
                objects: new_state.objects.values().cloned().collect(),
                active_users: new_state
                    .active_users
                    .iter()
                    .map(|u| UserInfo {
                        id: u.id.clone(),
                        name: u.name.clone(),
                        cursor_position: None,
                        color: [0.0, 0.0, 0.0, 1.0],
                        last_activity: chrono::Utc::now().timestamp_millis() as u64,
                        role: shared_types::UserRole::Viewer,
                        selected_objects: Vec::new(),
                    })
                    .collect(),
                timestamp: new_state.modified_at,
            };

            let message = BroadcastMessage::StateUpdate {
                session_id: uuid::Uuid::parse_str(session_id).map_err(|e| {
                    SessionError::InvalidInput {
                        field: format!("Invalid session_id UUID: {}", e),
                    }
                })?,
                state: snapshot,
            };

            return self
                .broadcast_to_session(session_id, message)
                .await
                .map_err(|_| SessionError::PersistenceError {
                    reason: "Failed to broadcast state update".to_string(),
                });
        }

        // Get or create delta tracker
        let mut trackers = self.delta_trackers.write().await;
        let tracker = trackers.entry(session_id.to_string()).or_insert_with(|| {
            DeltaTracker::new(uuid::Uuid::parse_str(session_id).unwrap_or_default())
        });

        // Compute delta
        let delta = tracker.compute_delta(old_state, new_state)?;

        if self.config.batch_deltas {
            // Add to pending deltas
            let mut pending = self.pending_deltas.write().await;
            pending
                .entry(session_id.to_string())
                .or_insert_with(Vec::new)
                .push(delta);

            // Schedule batch processing
            let manager = self.clone();
            let session_id = session_id.to_string();
            tokio::spawn(async move {
                tokio::time::sleep(tokio::time::Duration::from_millis(
                    manager.config.delta_batch_window_ms,
                ))
                .await;
                manager.flush_pending_deltas(&session_id).await.ok();
            });

            Ok(0) // Batched, not sent yet
        } else {
            // Send immediately
            self.send_delta_update(session_id, delta).await
        }
    }

    /// Flush pending deltas for a session
    async fn flush_pending_deltas(&self, session_id: &str) -> Result<usize, SessionError> {
        let mut pending = self.pending_deltas.write().await;

        if let Some(deltas) = pending.remove(session_id) {
            if deltas.is_empty() {
                return Ok(0);
            }

            // Batch multiple deltas
            if let Some(batched) = crate::delta::batch_deltas(deltas) {
                self.send_delta_update(session_id, batched).await
            } else {
                Ok(0)
            }
        } else {
            Ok(0)
        }
    }

    /// Send delta update
    async fn send_delta_update(
        &self,
        session_id: &str,
        delta: SessionDelta,
    ) -> Result<usize, SessionError> {
        let message = if self.config.compress_messages {
            // Compress delta
            let compressed = compression::compress_delta(&delta)?;
            let original_size = serde_json::to_vec(&delta)
                .map_err(|e| SessionError::PersistenceError {
                    reason: format!("Failed to serialize delta: {}", e),
                })?
                .len();

            BroadcastMessage::CompressedDeltaUpdate {
                session_id: uuid::Uuid::parse_str(session_id).map_err(|e| {
                    SessionError::InvalidInput {
                        field: format!("Invalid session_id UUID: {}", e),
                    }
                })?,
                data: compressed,
                original_size,
            }
        } else {
            BroadcastMessage::DeltaUpdate {
                session_id: uuid::Uuid::parse_str(session_id).map_err(|e| {
                    SessionError::InvalidInput {
                        field: format!("Invalid session_id UUID: {}", e),
                    }
                })?,
                delta,
                compressed: false,
            }
        };

        self.broadcast_to_session(session_id, message)
            .await
            .map_err(|_| SessionError::PersistenceError {
                reason: "Failed to broadcast delta update".to_string(),
            })
    }

    /// Create snapshot for new clients
    pub async fn create_snapshot(
        &self,
        session_id: &str,
        state: &SessionState,
    ) -> Result<SessionDelta, SessionError> {
        let mut trackers = self.delta_trackers.write().await;
        let tracker = trackers.entry(session_id.to_string()).or_insert_with(|| {
            DeltaTracker::new(uuid::Uuid::parse_str(session_id).unwrap_or_default())
        });

        tracker.create_snapshot(state)
    }
}

impl Default for BroadcastManager {
    fn default() -> Self {
        Self::new()
    }
}
