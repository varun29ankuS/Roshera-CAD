//! Real-time collaboration features
//!
//! Handles multi-user interactions, conflict resolution, and collaborative editing.

use serde::{Deserialize, Serialize};
use shared_types::session::UserInfo;
use shared_types::*;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Collaboration manager
#[derive(Clone)]
pub struct CollaborationManager {
    trackers: Arc<RwLock<HashMap<String, CollaborationTracker>>>,
}

impl CollaborationManager {
    /// Create new collaboration manager
    pub fn new() -> Self {
        Self {
            trackers: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Add user to session
    pub async fn add_user_to_session(&self, session_id: ObjectId, user: UserInfo) {
        let mut trackers = self.trackers.write().await;
        let tracker = trackers
            .entry(session_id.to_string())
            .or_insert_with(CollaborationTracker::new);

        tracker
            .user_activities
            .write()
            .await
            .insert(user.id.clone(), UserActivity::Idle);
    }
}

impl Default for CollaborationManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Tracks user activities in a session
#[derive(Debug, Clone)]
pub struct CollaborationTracker {
    /// User ID -> Current activity
    user_activities: Arc<RwLock<HashMap<String, UserActivity>>>,
    /// Object ID -> User ID (who is editing)
    object_locks: Arc<RwLock<HashMap<String, String>>>,
}

/// Represents what a user is currently doing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UserActivity {
    Idle,
    ViewingObject(String),
    EditingObject(String),
    CreatingObject,
    DeletingObject(String),
}

/// Collaboration event for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollaborationEvent {
    pub user_id: String,
    pub timestamp: u64,
    pub event_type: CollaborationEventType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CollaborationEventType {
    ObjectLocked { object_id: String },
    ObjectUnlocked { object_id: String },
    ObjectCreated { object_id: String },
    ObjectModified { object_id: String },
    ObjectDeleted { object_id: String },
}

impl CollaborationTracker {
    /// Creates a new collaboration tracker
    pub fn new() -> Self {
        Self {
            user_activities: Arc::new(RwLock::new(HashMap::new())),
            object_locks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Updates a user's activity
    pub async fn update_user_activity(&self, user_id: String, activity: UserActivity) {
        let mut activities = self.user_activities.write().await;
        activities.insert(user_id, activity);
    }

    /// Attempts to lock an object for editing
    pub async fn lock_object(
        &self,
        object_id: String,
        user_id: String,
    ) -> Result<(), CollaborationError> {
        let mut locks = self.object_locks.write().await;

        if let Some(existing_user) = locks.get(&object_id) {
            if existing_user != &user_id {
                return Err(CollaborationError::ObjectLocked {
                    object_id,
                    locked_by: existing_user.clone(),
                });
            }
        }

        locks.insert(object_id, user_id);
        Ok(())
    }

    /// Unlocks an object
    pub async fn unlock_object(
        &self,
        object_id: &str,
        user_id: &str,
    ) -> Result<(), CollaborationError> {
        let mut locks = self.object_locks.write().await;

        if let Some(lock_user) = locks.get(object_id) {
            if lock_user != user_id {
                return Err(CollaborationError::UnauthorizedUnlock);
            }
        }

        locks.remove(object_id);
        Ok(())
    }

    /// Checks if an object is locked
    pub async fn is_object_locked(&self, object_id: &str) -> bool {
        let locks = self.object_locks.read().await;
        locks.contains_key(object_id)
    }

    /// Gets the user who has locked an object
    pub async fn get_object_lock_owner(&self, object_id: &str) -> Option<String> {
        let locks = self.object_locks.read().await;
        locks.get(object_id).cloned()
    }

    /// Removes all locks for a user (when they disconnect)
    pub async fn remove_user_locks(&self, user_id: &str) {
        let mut locks = self.object_locks.write().await;
        locks.retain(|_, lock_user| lock_user != user_id);

        let mut activities = self.user_activities.write().await;
        activities.remove(user_id);
    }

    /// Gets all active users and their activities
    pub async fn get_active_users(&self) -> HashMap<String, UserActivity> {
        let activities = self.user_activities.read().await;
        activities.clone()
    }
}

impl Default for CollaborationTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// Collaboration-related errors
#[derive(Debug, thiserror::Error)]
pub enum CollaborationError {
    #[error("Object {object_id} is locked by {locked_by}")]
    ObjectLocked {
        object_id: String,
        locked_by: String,
    },

    #[error("Unauthorized attempt to unlock object")]
    UnauthorizedUnlock,

    #[error("Collaboration conflict")]
    Conflict,
}
