//! Storage layer for timeline persistence

use crate::{
    Branch, BranchId, Checkpoint, CheckpointId, EventId, StorageConfig, TimelineError,
    TimelineEvent, TimelineResult,
};
use dashmap::DashMap;
use std::sync::Arc;
use tokio::fs;

mod event_log;
mod index;
mod snapshot;

pub use event_log::EventLog;
pub use index::StorageIndex;
pub use snapshot::{Snapshot, SnapshotManager};

/// Main storage engine for timeline data
pub struct StorageEngine {
    /// Configuration
    config: StorageConfig,

    /// Event log for append-only storage
    event_log: Arc<EventLog>,

    /// Snapshot manager
    snapshot_manager: Arc<SnapshotManager>,

    /// Storage index for fast lookups
    index: Arc<StorageIndex>,

    /// Write locks per branch (to ensure append-only)
    branch_locks: Arc<DashMap<BranchId, Arc<tokio::sync::Mutex<()>>>>,
}

impl StorageEngine {
    /// Create a new storage engine
    pub async fn new(config: StorageConfig) -> TimelineResult<Self> {
        // Ensure base directory exists
        fs::create_dir_all(&config.base_path)
            .await
            .map_err(TimelineError::StorageError)?;

        // Initialize components
        let event_log = Arc::new(EventLog::new(&config).await?);
        let snapshot_manager = Arc::new(SnapshotManager::new(&config).await?);
        let index = Arc::new(StorageIndex::new(&config).await?);

        Ok(Self {
            config,
            event_log,
            snapshot_manager,
            index,
            branch_locks: Arc::new(DashMap::new()),
        })
    }

    /// Persist an event
    pub async fn persist_event(
        &self,
        event: &TimelineEvent,
        timeline: Arc<crate::Timeline>,
    ) -> TimelineResult<()> {
        // Get branch lock
        let lock = self
            .branch_locks
            .entry(event.metadata.branch_id)
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone();

        let _guard = lock.lock().await;

        // Append to event log
        self.event_log.append(event).await?;

        // Update index
        self.index.index_event(event).await?;

        // Check if we need a snapshot
        if self.should_create_snapshot(event.sequence_number) {
            // Trigger snapshot in background
            let snapshot_manager = self.snapshot_manager.clone();
            let event_id = event.id;
            let timeline_clone = timeline.clone();
            tokio::spawn(async move {
                if let Err(e) = snapshot_manager
                    .create_snapshot_at(event_id, timeline_clone)
                    .await
                {
                    tracing::error!("Failed to create snapshot: {}", e);
                }
            });
        }

        Ok(())
    }

    /// Load an event by ID
    pub async fn load_event(&self, event_id: EventId) -> TimelineResult<TimelineEvent> {
        // Check index for location
        let location = self.index.get_event_location(event_id).await?;

        // Load from event log
        self.event_log.read_event(location).await
    }

    /// Load events in a range
    pub async fn load_events_range(
        &self,
        branch_id: BranchId,
        start: u64,
        end: u64,
    ) -> TimelineResult<Vec<TimelineEvent>> {
        let locations = self.index.get_branch_events(branch_id, start, end).await?;

        let mut events = Vec::new();
        for location in locations {
            events.push(self.event_log.read_event(location).await?);
        }

        Ok(events)
    }

    /// Persist a branch
    pub async fn persist_branch(&self, branch: &Branch) -> TimelineResult<()> {
        let branch_path = self.config.base_path.join("branches");
        fs::create_dir_all(&branch_path)
            .await
            .map_err(TimelineError::StorageError)?;

        let file_path = branch_path.join(format!("{}.branch", branch.id.0));
        let data = rmp_serde::to_vec(branch)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        let compressed = if self.config.compression_enabled {
            lz4_flex::compress_prepend_size(&data)
        } else {
            data
        };

        fs::write(&file_path, compressed)
            .await
            .map_err(TimelineError::StorageError)?;

        Ok(())
    }

    /// Load a branch
    pub async fn load_branch(&self, branch_id: BranchId) -> TimelineResult<Branch> {
        let file_path = self
            .config
            .base_path
            .join("branches")
            .join(format!("{}.branch", branch_id.0));

        let compressed = fs::read(&file_path)
            .await
            .map_err(TimelineError::StorageError)?;

        let data = if self.config.compression_enabled {
            lz4_flex::decompress_size_prepended(&compressed).map_err(|e| {
                TimelineError::StorageError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Decompression failed: {}", e),
                ))
            })?
        } else {
            compressed
        };

        rmp_serde::from_slice(&data).map_err(|e| TimelineError::SerializationError(e.to_string()))
    }

    /// Persist a checkpoint
    pub async fn persist_checkpoint(&self, checkpoint: &Checkpoint) -> TimelineResult<()> {
        let checkpoint_path = self.config.base_path.join("checkpoints");
        fs::create_dir_all(&checkpoint_path)
            .await
            .map_err(TimelineError::StorageError)?;

        let file_path = checkpoint_path.join(format!("{}.checkpoint", checkpoint.id.0));
        let data = serde_json::to_vec_pretty(checkpoint).map_err(TimelineError::JsonError)?;

        fs::write(&file_path, data)
            .await
            .map_err(TimelineError::StorageError)?;

        Ok(())
    }

    /// Load a checkpoint
    pub async fn load_checkpoint(&self, checkpoint_id: CheckpointId) -> TimelineResult<Checkpoint> {
        let file_path = self
            .config
            .base_path
            .join("checkpoints")
            .join(format!("{}.checkpoint", checkpoint_id.0));

        let data = fs::read(&file_path)
            .await
            .map_err(TimelineError::StorageError)?;

        serde_json::from_slice(&data).map_err(TimelineError::JsonError)
    }

    /// Get the latest snapshot for a branch
    pub async fn get_latest_snapshot(
        &self,
        branch_id: BranchId,
    ) -> TimelineResult<Option<Snapshot>> {
        self.snapshot_manager.get_latest_snapshot(branch_id).await
    }

    /// Clean up old data
    pub async fn cleanup(&self) -> TimelineResult<()> {
        // Clean old snapshots
        self.snapshot_manager.cleanup_old_snapshots().await?;

        // Compact event log if needed
        self.event_log.compact_if_needed().await?;

        Ok(())
    }

    /// Check if we should create a snapshot
    fn should_create_snapshot(&self, sequence_number: u64) -> bool {
        sequence_number % self.config.snapshot_interval as u64 == 0
    }

    /// Get storage statistics
    pub async fn get_stats(&self) -> TimelineResult<StorageStats> {
        let events_size = self.event_log.get_size().await?;
        let snapshots_size = self.snapshot_manager.get_total_size().await?;
        let index_size = self.index.get_size().await?;

        Ok(StorageStats {
            total_events: self.index.get_event_count().await?,
            total_snapshots: self.snapshot_manager.get_snapshot_count().await?,
            events_size_bytes: events_size,
            snapshots_size_bytes: snapshots_size,
            index_size_bytes: index_size,
            total_size_bytes: events_size + snapshots_size + index_size,
        })
    }
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    /// Total number of events
    pub total_events: u64,
    /// Total number of snapshots
    pub total_snapshots: u64,
    /// Size of event logs in bytes
    pub events_size_bytes: u64,
    /// Size of snapshots in bytes
    pub snapshots_size_bytes: u64,
    /// Size of indexes in bytes
    pub index_size_bytes: u64,
    /// Total storage size in bytes
    pub total_size_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn create_test_storage() -> (StorageEngine, TempDir) {
        let temp_dir = TempDir::new().unwrap();
        let config = StorageConfig {
            base_path: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let storage = StorageEngine::new(config).await.unwrap();
        (storage, temp_dir)
    }

    #[tokio::test]
    async fn test_storage_creation() {
        let (storage, _temp_dir) = create_test_storage().await;
        let stats = storage.get_stats().await.unwrap();
        assert_eq!(stats.total_events, 0);
    }
}
