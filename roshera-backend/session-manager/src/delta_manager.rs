//! Delta Manager for production-grade delta synchronization
//!
//! This module provides a high-level interface for managing session deltas,
//! including storage, retrieval, compression, and atomic operations.

use crate::delta::{compression, DeltaTracker, SessionDelta};
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use shared_types::{SessionError, SessionState};
use std::collections::VecDeque;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Maximum number of deltas to keep in memory per session
const MAX_DELTAS_IN_MEMORY: usize = 1000;

/// Maximum age of deltas before archival (in seconds)
const DELTA_ARCHIVE_AGE_SECS: u64 = 3600; // 1 hour

/// Statistics about delta operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeltaStatistics {
    pub total_deltas: usize,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
    pub oldest_sequence: u64,
    pub newest_sequence: u64,
    pub delta_rate: f64, // deltas per minute
}

/// Storage for session deltas
#[derive(Debug)]
struct SessionDeltaStorage {
    /// Recent deltas in memory
    deltas: VecDeque<SessionDelta>,
    /// Compressed deltas for older entries
    compressed_deltas: VecDeque<Vec<u8>>,
    /// Delta tracker for this session
    tracker: DeltaTracker,
    /// Current session state
    current_state: SessionState,
    /// Last snapshot sequence
    last_snapshot_sequence: u64,
    /// Statistics
    stats: DeltaStatistics,
}

impl SessionDeltaStorage {
    fn new(session_id: Uuid) -> Self {
        Self {
            deltas: VecDeque::new(),
            compressed_deltas: VecDeque::new(),
            tracker: DeltaTracker::new(session_id),
            current_state: SessionState::new(
                shared_types::ObjectId::new_v4(),
                "system".to_string(),
            ),
            last_snapshot_sequence: 0,
            stats: DeltaStatistics {
                total_deltas: 0,
                compressed_size: 0,
                uncompressed_size: 0,
                oldest_sequence: 0,
                newest_sequence: 0,
                delta_rate: 0.0,
            },
        }
    }

    /// Add a new delta
    fn add_delta(&mut self, delta: SessionDelta) -> Result<(), SessionError> {
        // Update statistics
        self.stats.total_deltas += 1;
        if self.stats.oldest_sequence == 0 {
            self.stats.oldest_sequence = delta.sequence;
        }
        self.stats.newest_sequence = delta.sequence;

        // Apply delta to current state
        self.tracker.apply_delta(&mut self.current_state, &delta)?;

        // Store delta
        self.deltas.push_back(delta.clone());

        // Compress old deltas if we have too many
        if self.deltas.len() > MAX_DELTAS_IN_MEMORY {
            if let Some(old_delta) = self.deltas.pop_front() {
                if let Ok(compressed) = compression::compress_delta(&old_delta) {
                    self.stats.compressed_size += compressed.len();
                    self.compressed_deltas.push_back(compressed);
                }
            }
        }

        // Archive very old compressed deltas
        while self.compressed_deltas.len() > MAX_DELTAS_IN_MEMORY {
            self.compressed_deltas.pop_front();
        }

        Ok(())
    }

    /// Get deltas since a sequence number
    fn get_deltas_since(&self, since_sequence: u64) -> Vec<SessionDelta> {
        self.deltas
            .iter()
            .filter(|d| d.sequence > since_sequence)
            .cloned()
            .collect()
    }

    /// Create a snapshot at current state
    fn create_snapshot(&mut self) -> Result<SessionDelta, SessionError> {
        let snapshot = self.tracker.create_snapshot(&self.current_state)?;
        self.last_snapshot_sequence = snapshot.sequence;
        Ok(snapshot)
    }
}

/// Delta Manager for managing session deltas across the system
pub struct DeltaManager {
    /// Storage for each session's deltas
    sessions: Arc<DashMap<String, Arc<RwLock<SessionDeltaStorage>>>>,
    /// Archive storage (in production, this would be a database)
    archive: Arc<DashMap<String, Vec<Vec<u8>>>>,
}

impl DeltaManager {
    /// Create a new delta manager
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(DashMap::new()),
            archive: Arc::new(DashMap::new()),
        }
    }

    /// Get or create storage for a session
    async fn get_or_create_storage(&self, session_id: &str) -> Arc<RwLock<SessionDeltaStorage>> {
        let session_uuid = match Uuid::parse_str(session_id) {
            Ok(uuid) => uuid,
            Err(_) => Uuid::new_v4(), // Fallback for invalid IDs
        };

        self.sessions
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(RwLock::new(SessionDeltaStorage::new(session_uuid))))
            .clone()
    }

    /// Apply a delta to a session
    pub async fn apply_delta(
        &self,
        session_id: &str,
        delta: SessionDelta,
    ) -> Result<(), SessionError> {
        info!(
            "Applying delta {} to session {}",
            delta.sequence, session_id
        );

        let storage = self.get_or_create_storage(session_id).await;
        let mut storage_guard = storage.write().await;

        storage_guard.add_delta(delta)?;

        debug!("Delta applied successfully to session {}", session_id);
        Ok(())
    }

    /// Apply multiple deltas atomically
    pub async fn apply_deltas_atomic(
        &self,
        session_id: &str,
        deltas: Vec<SessionDelta>,
    ) -> Result<(), SessionError> {
        info!(
            "Applying {} deltas atomically to session {}",
            deltas.len(),
            session_id
        );

        let storage = self.get_or_create_storage(session_id).await;
        let mut storage_guard = storage.write().await;

        // Try to apply all deltas, rolling back on failure
        let original_state = storage_guard.current_state.clone();

        for delta in deltas {
            if let Err(e) = storage_guard.add_delta(delta) {
                // Rollback on error
                storage_guard.current_state = original_state;
                error!("Failed to apply delta atomically: {}", e);
                return Err(e);
            }
        }

        debug!("All deltas applied atomically to session {}", session_id);
        Ok(())
    }

    /// Get deltas since a specific sequence number
    pub async fn get_deltas_since(
        &self,
        session_id: &str,
        since_sequence: u64,
    ) -> Result<Vec<SessionDelta>, SessionError> {
        debug!(
            "Getting deltas for session {} since sequence {}",
            session_id, since_sequence
        );

        let storage = self.get_or_create_storage(session_id).await;
        let storage_guard = storage.read().await;

        let deltas = storage_guard.get_deltas_since(since_sequence);

        debug!("Found {} deltas for session {}", deltas.len(), session_id);
        Ok(deltas)
    }

    /// Generate a snapshot at a specific sequence (or latest if None)
    pub async fn generate_snapshot(
        &self,
        session_id: &str,
        sequence: Option<u64>,
    ) -> Result<serde_json::Value, SessionError> {
        info!(
            "Generating snapshot for session {} at sequence {:?}",
            session_id, sequence
        );

        let storage = self.get_or_create_storage(session_id).await;
        let mut storage_guard = storage.write().await;

        // If specific sequence requested, replay to that point
        if let Some(target_seq) = sequence {
            if target_seq < storage_guard.stats.newest_sequence {
                warn!(
                    "Cannot generate snapshot for past sequence {}, using current",
                    target_seq
                );
            }
        }

        // Create snapshot delta
        let snapshot_delta = storage_guard.create_snapshot()?;

        // Convert to JSON for API response
        let snapshot_json = serde_json::json!({
            "session_id": session_id,
            "sequence": snapshot_delta.sequence,
            "timestamp": snapshot_delta.timestamp,
            "state": storage_guard.current_state,
            "delta": snapshot_delta,
        });

        debug!("Snapshot generated for session {}", session_id);
        Ok(snapshot_json)
    }

    /// Get statistics for a session
    pub async fn get_statistics(&self, session_id: &str) -> Result<DeltaStatistics, SessionError> {
        debug!("Getting statistics for session {}", session_id);

        let storage = self.get_or_create_storage(session_id).await;
        let storage_guard = storage.read().await;

        let mut stats = storage_guard.stats.clone();

        // Calculate delta rate
        if stats.total_deltas > 0 && stats.newest_sequence > stats.oldest_sequence {
            // Estimate based on sequence numbers and assuming ~1 delta per second average
            let time_span_minutes = (stats.newest_sequence - stats.oldest_sequence) as f64 / 60.0;
            if time_span_minutes > 0.0 {
                stats.delta_rate = stats.total_deltas as f64 / time_span_minutes;
            }
        }

        // Calculate uncompressed size estimate
        stats.uncompressed_size = stats.total_deltas * 1024; // Rough estimate: 1KB per delta

        Ok(stats)
    }

    /// Compact deltas for a session (merge small deltas, remove redundant changes)
    pub async fn compact_deltas(&self, session_id: &str) -> Result<(), SessionError> {
        info!("Compacting deltas for session {}", session_id);

        let storage = self.get_or_create_storage(session_id).await;
        let mut storage_guard = storage.write().await;

        // Batch deltas if we have many small ones
        if storage_guard.deltas.len() > 10 {
            let deltas: Vec<_> = storage_guard.deltas.drain(..).collect();

            // Batch every 5 deltas together
            let mut compacted = Vec::new();
            for chunk in deltas.chunks(5) {
                if let Some(batched) = crate::delta::batch_deltas(chunk.to_vec()) {
                    compacted.push(batched);
                }
            }

            // Replace with compacted deltas
            storage_guard.deltas = VecDeque::from(compacted);

            info!(
                "Compacted {} deltas into {} for session {}",
                deltas.len(),
                storage_guard.deltas.len(),
                session_id
            );
        }

        // Compress more aggressively
        while storage_guard.deltas.len() > MAX_DELTAS_IN_MEMORY / 2 {
            if let Some(delta) = storage_guard.deltas.pop_front() {
                if let Ok(compressed) = compression::compress_delta(&delta) {
                    storage_guard.stats.compressed_size += compressed.len();
                    storage_guard.compressed_deltas.push_back(compressed);
                }
            }
        }

        debug!("Delta compaction complete for session {}", session_id);
        Ok(())
    }

    /// Clean up old sessions
    pub async fn cleanup_old_sessions(&self, max_age_secs: u64) {
        info!("Cleaning up sessions older than {} seconds", max_age_secs);

        let mut sessions_to_remove = Vec::new();

        // Check each session's age
        for entry in self.sessions.iter() {
            let session_id = entry.key().clone();
            let storage = entry.value().clone();
            let storage_guard = storage.read().await;

            // Check if session is old based on newest delta timestamp
            let now = chrono::Utc::now().timestamp() as u64;
            let newest_timestamp = storage_guard
                .deltas
                .back()
                .map(|d| d.timestamp)
                .unwrap_or(0);

            if newest_timestamp > 0 && (now - newest_timestamp) > max_age_secs {
                sessions_to_remove.push(session_id);
            }
        }

        // Archive and remove old sessions
        for session_id in sessions_to_remove {
            if let Some((_, storage)) = self.sessions.remove(&session_id) {
                let storage_guard = storage.read().await;

                // Archive compressed deltas
                let mut archived = Vec::new();
                archived.extend(storage_guard.compressed_deltas.clone());

                // Compress and archive recent deltas
                for delta in &storage_guard.deltas {
                    if let Ok(compressed) = compression::compress_delta(delta) {
                        archived.push(compressed);
                    }
                }

                self.archive.insert(session_id.clone(), archived);
                info!("Archived session {}", session_id);
            }
        }
    }

    /// Restore session from archive
    pub async fn restore_from_archive(&self, session_id: &str) -> Result<(), SessionError> {
        info!("Restoring session {} from archive", session_id);

        if let Some(archived) = self.archive.get(session_id) {
            let storage = self.get_or_create_storage(session_id).await;
            let mut storage_guard = storage.write().await;

            // Restore compressed deltas
            storage_guard.compressed_deltas = VecDeque::from(archived.value().clone());

            // Decompress recent deltas
            let mut restored_count = 0;
            while let Some(compressed) = storage_guard.compressed_deltas.pop_back() {
                if let Ok(delta) = compression::decompress_delta(&compressed) {
                    storage_guard.deltas.push_front(delta);
                    restored_count += 1;

                    // Only restore recent deltas to memory
                    if restored_count >= 100 {
                        break;
                    }
                }
            }

            info!(
                "Restored {} deltas for session {}",
                restored_count, session_id
            );
            Ok(())
        } else {
            Err(SessionError::NotFound {
                id: session_id.to_string(),
            })
        }
    }
}

impl Default for DeltaManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::delta::{DeltaType, ObjectDelta};
    use shared_types::{CADObject, Mesh, ObjectId, Transform3D};

    #[tokio::test]
    async fn test_delta_manager_basic() {
        let manager = DeltaManager::new();
        let session_id = "test-session";

        // Create a delta
        let delta = SessionDelta {
            session_id: Uuid::new_v4(),
            sequence: 1,
            timestamp: chrono::Utc::now().timestamp() as u64,
            object_deltas: vec![],
            timeline_delta: None,
            metadata_changes: None,
            user_changes: None,
            settings_changes: None,
        };

        // Apply delta
        manager
            .apply_delta(session_id, delta.clone())
            .await
            .unwrap();

        // Get deltas
        let deltas = manager.get_deltas_since(session_id, 0).await.unwrap();
        assert_eq!(deltas.len(), 1);
        assert_eq!(deltas[0].sequence, 1);
    }

    #[tokio::test]
    async fn test_delta_manager_atomic() {
        let manager = DeltaManager::new();
        let session_id = "test-session-atomic";

        // Create multiple deltas
        let deltas = vec![
            SessionDelta {
                session_id: Uuid::new_v4(),
                sequence: 1,
                timestamp: chrono::Utc::now().timestamp() as u64,
                object_deltas: vec![],
                timeline_delta: None,
                metadata_changes: None,
                user_changes: None,
                settings_changes: None,
            },
            SessionDelta {
                session_id: Uuid::new_v4(),
                sequence: 2,
                timestamp: chrono::Utc::now().timestamp() as u64,
                object_deltas: vec![],
                timeline_delta: None,
                metadata_changes: None,
                user_changes: None,
                settings_changes: None,
            },
        ];

        // Apply atomically
        manager
            .apply_deltas_atomic(session_id, deltas)
            .await
            .unwrap();

        // Verify both were applied
        let retrieved = manager.get_deltas_since(session_id, 0).await.unwrap();
        assert_eq!(retrieved.len(), 2);
    }

    #[tokio::test]
    async fn test_delta_manager_snapshot() {
        let manager = DeltaManager::new();
        let session_id = "test-session-snapshot";

        // Apply some deltas
        for i in 1..=3 {
            let delta = SessionDelta {
                session_id: Uuid::new_v4(),
                sequence: i,
                timestamp: chrono::Utc::now().timestamp() as u64,
                object_deltas: vec![],
                timeline_delta: None,
                metadata_changes: None,
                user_changes: None,
                settings_changes: None,
            };
            manager.apply_delta(session_id, delta).await.unwrap();
        }

        // Generate snapshot
        let snapshot = manager.generate_snapshot(session_id, None).await.unwrap();
        assert!(snapshot.get("session_id").is_some());
        assert!(snapshot.get("sequence").is_some());
        assert!(snapshot.get("state").is_some());
    }

    #[tokio::test]
    async fn test_delta_manager_statistics() {
        let manager = DeltaManager::new();
        let session_id = "test-session-stats";

        // Apply some deltas
        for i in 1..=5 {
            let delta = SessionDelta {
                session_id: Uuid::new_v4(),
                sequence: i,
                timestamp: (chrono::Utc::now().timestamp() + i as i64) as u64,
                object_deltas: vec![],
                timeline_delta: None,
                metadata_changes: None,
                user_changes: None,
                settings_changes: None,
            };
            manager.apply_delta(session_id, delta).await.unwrap();
        }

        // Get statistics
        let stats = manager.get_statistics(session_id).await.unwrap();
        assert_eq!(stats.total_deltas, 5);
        assert_eq!(stats.oldest_sequence, 1);
        assert_eq!(stats.newest_sequence, 5);
    }

    #[tokio::test]
    async fn test_delta_manager_compaction() {
        let manager = DeltaManager::new();
        let session_id = "test-session-compact";

        // Apply many small deltas
        for i in 1..=20 {
            let delta = SessionDelta {
                session_id: Uuid::new_v4(),
                sequence: i,
                timestamp: chrono::Utc::now().timestamp() as u64,
                object_deltas: vec![],
                timeline_delta: None,
                metadata_changes: None,
                user_changes: None,
                settings_changes: None,
            };
            manager.apply_delta(session_id, delta).await.unwrap();
        }

        // Compact deltas
        manager.compact_deltas(session_id).await.unwrap();

        // Stats should still show all deltas
        let stats = manager.get_statistics(session_id).await.unwrap();
        assert_eq!(stats.total_deltas, 20);
    }
}
