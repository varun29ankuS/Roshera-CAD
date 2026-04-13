//! Session state management
//!
//! Manages the current state of CAD sessions including objects, users, and history.

use shared_types::*;
use std::sync::Arc;
use tokio::sync::RwLock;

// Remove the duplicate SessionState struct - just use the one from shared_types

/// Thread-safe session state container
pub type SharedSessionState = Arc<RwLock<SessionState>>;

/// Extension methods for SessionState
pub trait SessionStateExt {
    /// Update the modified timestamp
    fn update_modified_timestamp(&mut self);
}

impl SessionStateExt for SessionState {
    fn update_modified_timestamp(&mut self) {
        self.modified_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_else(|_| std::time::Duration::from_secs(0))
            .as_millis() as u64;
    }
}
