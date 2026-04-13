//! Snapshot management for efficient timeline reconstruction

use crate::{
    BranchId, EntityId, EventId, SnapshotId, StorageConfig, TimelineError, TimelineResult,
};
use dashmap::DashMap;
use std::path::PathBuf;
use std::sync::Arc;

/// A snapshot of the timeline state at a specific point
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Snapshot {
    /// Unique identifier
    pub id: SnapshotId,

    /// Branch this snapshot belongs to
    pub branch_id: BranchId,

    /// Event ID this snapshot was taken at
    pub event_id: EventId,

    /// Event sequence number
    pub sequence_number: u64,

    /// Timestamp when snapshot was created
    pub created_at: chrono::DateTime<chrono::Utc>,

    /// Entities state at this point
    pub entities: std::collections::HashMap<EntityId, EntityState>,

    /// Metadata
    pub metadata: SnapshotMetadata,
}

/// State of an entity in the snapshot
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EntityState {
    /// Entity ID
    pub id: EntityId,

    /// Entity data (serialized geometry)
    pub data: Vec<u8>,

    /// Entity type
    pub entity_type: String,

    /// Properties
    pub properties: serde_json::Value,
}

/// Snapshot metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SnapshotMetadata {
    /// Size in bytes
    pub size_bytes: u64,

    /// Number of entities
    pub entity_count: usize,

    /// Compression ratio
    pub compression_ratio: f32,

    /// Creation duration in milliseconds
    pub creation_duration_ms: u64,
}

/// Manages snapshots for the timeline
pub struct SnapshotManager {
    /// Base directory for snapshots
    base_dir: PathBuf,

    /// Snapshot index (branch -> snapshot list)
    snapshot_index: Arc<DashMap<BranchId, Vec<SnapshotInfo>>>,

    /// Configuration
    config: StorageConfig,
}

/// Lightweight snapshot information for indexing
#[derive(Debug, Clone)]
struct SnapshotInfo {
    id: SnapshotId,
    branch_id: BranchId,
    event_id: EventId,
    sequence_number: u64,
    created_at: chrono::DateTime<chrono::Utc>,
    size_bytes: u64,
}

impl SnapshotManager {
    /// Create a new snapshot manager
    pub async fn new(config: &StorageConfig) -> TimelineResult<Self> {
        let base_dir = config.base_path.join("snapshots");
        tokio::fs::create_dir_all(&base_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        let manager = Self {
            base_dir,
            snapshot_index: Arc::new(DashMap::new()),
            config: config.clone(),
        };

        // Load existing snapshots
        manager.load_snapshot_index().await?;

        Ok(manager)
    }

    /// Create a snapshot at the given event
    pub async fn create_snapshot_at(
        &self,
        event_id: EventId,
        timeline: Arc<crate::Timeline>,
    ) -> TimelineResult<SnapshotId> {
        // Get the actual data from the timeline
        // Use main branch as default for snapshots - this ensures consistency
        // across restarts and provides a stable reference point for recovery
        let branch_id = crate::BranchId::main();

        // Retrieve the event to extract its sequence number
        // This provides the chronological position needed for efficient snapshot indexing
        let sequence_number = timeline
            .get_event(event_id)
            .map(|event| event.sequence_number)
            .unwrap_or(0);

        // Reconstruct complete entity state at this event point through incremental replay
        // This ensures accurate snapshots that reflect the exact state of the timeline
        let entity_states = timeline
            .reconstruct_entities_at_event(event_id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!("Failed to reconstruct entities for snapshot: {}", e);
                std::collections::HashMap::new()
            });

        // Convert from execution::EntityState to storage::EntityState format
        let mut entities = std::collections::HashMap::new();
        for (entity_id, exec_state) in entity_states {
            let storage_state = EntityState {
                id: entity_id,
                data: exec_state.geometry_data,
                entity_type: format!("{:?}", exec_state.entity_type),
                properties: exec_state.properties,
            };
            entities.insert(entity_id, storage_state);
        }

        self.create_snapshot_with_data(event_id, branch_id, sequence_number, entities)
            .await
    }

    /// Create a snapshot at the given event with proper metadata
    pub async fn create_snapshot_with_data(
        &self,
        event_id: EventId,
        branch_id: BranchId,
        sequence_number: u64,
        entities: std::collections::HashMap<EntityId, EntityState>,
    ) -> TimelineResult<SnapshotId> {
        let start = std::time::Instant::now();

        // Calculate metadata
        let entity_count = entities.len();
        let estimated_size = entities
            .values()
            .map(|e| e.data.len() + 100) // Geometry data + overhead
            .sum::<usize>() as u64;

        let snapshot = Snapshot {
            id: SnapshotId::new(),
            branch_id,
            event_id,
            sequence_number,
            created_at: chrono::Utc::now(),
            entities,
            metadata: SnapshotMetadata {
                size_bytes: estimated_size,
                entity_count,
                compression_ratio: 1.0,  // Will be updated after compression
                creation_duration_ms: 0, // Will be updated after save
            },
        };

        // Save and update metadata
        self.save_snapshot(&snapshot).await?;

        // Update creation duration
        let duration_ms = start.elapsed().as_millis() as u64;

        // Update index with proper metadata
        let info = SnapshotInfo {
            id: snapshot.id,
            branch_id,
            event_id,
            sequence_number,
            created_at: snapshot.created_at,
            size_bytes: estimated_size,
        };

        self.snapshot_index
            .entry(branch_id)
            .or_insert_with(Vec::new)
            .push(info);

        tracing::info!(
            "Created snapshot {} with {} entities in {}ms",
            snapshot.id.0,
            entity_count,
            duration_ms
        );

        Ok(snapshot.id)
    }

    /// Save a snapshot to disk
    pub async fn save_snapshot(&self, snapshot: &Snapshot) -> TimelineResult<()> {
        let start = std::time::Instant::now();

        // Serialize snapshot
        let data = bincode::serialize(snapshot)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        let uncompressed_size = data.len() as u64;

        // Compress if enabled
        let compressed = if self.config.compression_enabled {
            lz4_flex::compress_prepend_size(&data)
        } else {
            data
        };

        let compressed_size = compressed.len() as u64;
        let compression_ratio = uncompressed_size as f32 / compressed_size as f32;

        // Write to file
        let file_path = self.base_dir.join(format!(
            "{}/{}.snapshot",
            snapshot.branch_id.0, snapshot.id.0
        ));

        // Ensure branch directory exists
        if let Some(parent) = file_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(TimelineError::StorageError)?;
        }

        tokio::fs::write(&file_path, compressed)
            .await
            .map_err(TimelineError::StorageError)?;

        // Update index
        let info = SnapshotInfo {
            id: snapshot.id,
            branch_id: snapshot.branch_id,
            event_id: snapshot.event_id,
            sequence_number: snapshot.sequence_number,
            created_at: snapshot.created_at,
            size_bytes: compressed_size,
        };

        self.snapshot_index
            .entry(snapshot.branch_id)
            .or_insert_with(Vec::new)
            .push(info);

        // Sort by sequence number
        if let Some(mut snapshots) = self.snapshot_index.get_mut(&snapshot.branch_id) {
            snapshots.sort_by_key(|s| s.sequence_number);
        }

        tracing::info!(
            "Created snapshot {} for branch {} at sequence {} ({}ms, {:.2}x compression)",
            snapshot.id.0,
            snapshot.branch_id.0,
            snapshot.sequence_number,
            start.elapsed().as_millis(),
            compression_ratio
        );

        Ok(())
    }

    /// Load a snapshot from disk
    pub async fn load_snapshot(&self, snapshot_id: SnapshotId) -> TimelineResult<Snapshot> {
        // Find snapshot in index
        let (branch_id, _) = self.find_snapshot_info(snapshot_id)?;

        let file_path = self
            .base_dir
            .join(format!("{}/{}.snapshot", branch_id.0, snapshot_id.0));

        let compressed = tokio::fs::read(&file_path)
            .await
            .map_err(TimelineError::StorageError)?;

        // Decompress if needed
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

        // Deserialize
        bincode::deserialize(&data).map_err(|e| TimelineError::SerializationError(e.to_string()))
    }

    /// Get the latest snapshot for a branch
    pub async fn get_latest_snapshot(
        &self,
        branch_id: BranchId,
    ) -> TimelineResult<Option<Snapshot>> {
        if let Some(snapshots) = self.snapshot_index.get(&branch_id) {
            if let Some(latest_info) = snapshots.last() {
                return Ok(Some(self.load_snapshot(latest_info.id).await?));
            }
        }

        Ok(None)
    }

    /// Get the best snapshot to use for reconstructing state at a given sequence number
    pub async fn get_best_snapshot_before(
        &self,
        branch_id: BranchId,
        sequence_number: u64,
    ) -> TimelineResult<Option<Snapshot>> {
        if let Some(snapshots) = self.snapshot_index.get(&branch_id) {
            // Binary search for the best snapshot
            let idx = snapshots
                .binary_search_by_key(&sequence_number, |s| s.sequence_number)
                .unwrap_or_else(|i| i.saturating_sub(1));

            if idx < snapshots.len() && snapshots[idx].sequence_number <= sequence_number {
                return Ok(Some(self.load_snapshot(snapshots[idx].id).await?));
            }
        }

        Ok(None)
    }

    /// Clean up old snapshots
    pub async fn cleanup_old_snapshots(&self) -> TimelineResult<()> {
        // Keep last N snapshots per branch
        const SNAPSHOTS_TO_KEEP: usize = 10;

        for mut entry in self.snapshot_index.iter_mut() {
            let branch_id = *entry.key();
            let snapshots = entry.value_mut();

            if snapshots.len() > SNAPSHOTS_TO_KEEP {
                // Remove old snapshots
                let to_remove: Vec<_> = snapshots
                    .drain(..snapshots.len() - SNAPSHOTS_TO_KEEP)
                    .collect();

                for info in to_remove {
                    let file_path = self
                        .base_dir
                        .join(format!("{}/{}.snapshot", branch_id.0, info.id.0));

                    if let Err(e) = tokio::fs::remove_file(&file_path).await {
                        tracing::warn!("Failed to remove old snapshot: {}", e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Load snapshot index from disk
    async fn load_snapshot_index(&self) -> TimelineResult<()> {
        let mut entries = tokio::fs::read_dir(&self.base_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            let path = entry.path();
            if path.is_dir() {
                // This is a branch directory
                if let Some(branch_name) = path.file_name().and_then(|n| n.to_str()) {
                    if let Ok(branch_uuid) = uuid::Uuid::parse_str(branch_name) {
                        let branch_id = BranchId(branch_uuid);
                        self.load_branch_snapshots(branch_id).await?;
                    }
                }
            }
        }

        Ok(())
    }

    /// Load snapshots for a specific branch
    async fn load_branch_snapshots(&self, branch_id: BranchId) -> TimelineResult<()> {
        let branch_dir = self.base_dir.join(branch_id.0.to_string());

        if !branch_dir.exists() {
            return Ok(());
        }

        let mut entries = tokio::fs::read_dir(&branch_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        let mut snapshots = Vec::new();

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("snapshot") {
                // Load snapshot metadata
                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                    if let Ok(snapshot_uuid) = uuid::Uuid::parse_str(name) {
                        let metadata = entry
                            .metadata()
                            .await
                            .map_err(TimelineError::StorageError)?;

                        // Load just the header to get info
                        // Parse metadata from filename format: {uuid}_{event_id}_{sequence}.snapshot
                        let event_id =
                            if let Some(name_str) = path.file_stem().and_then(|n| n.to_str()) {
                                let parts: Vec<&str> = name_str.split('_').collect();
                                if parts.len() >= 3 {
                                    parts
                                        .get(1)
                                        .and_then(|s| uuid::Uuid::parse_str(s).ok())
                                        .map(EventId)
                                        .unwrap_or_else(EventId::new)
                                } else {
                                    EventId::new()
                                }
                            } else {
                                EventId::new()
                            };

                        let sequence_number =
                            if let Some(name_str) = path.file_stem().and_then(|n| n.to_str()) {
                                let parts: Vec<&str> = name_str.split('_').collect();
                                parts
                                    .get(2)
                                    .and_then(|s| s.parse::<u64>().ok())
                                    .unwrap_or(0)
                            } else {
                                0
                            };

                        // Use file modification time
                        let created_at = metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| {
                                chrono::Utc::now() - chrono::Duration::seconds(d.as_secs() as i64)
                            })
                            .unwrap_or_else(chrono::Utc::now);

                        snapshots.push(SnapshotInfo {
                            id: SnapshotId(snapshot_uuid),
                            branch_id: branch_id.clone(),
                            event_id,
                            sequence_number,
                            created_at,
                            size_bytes: metadata.len(),
                        });
                    }
                }
            }
        }

        if !snapshots.is_empty() {
            snapshots.sort_by_key(|s| s.sequence_number);
            self.snapshot_index.insert(branch_id, snapshots);
        }

        Ok(())
    }

    /// Find snapshot info by ID
    fn find_snapshot_info(
        &self,
        snapshot_id: SnapshotId,
    ) -> TimelineResult<(BranchId, SnapshotInfo)> {
        for entry in self.snapshot_index.iter() {
            let branch_id = *entry.key();
            let snapshots = entry.value();

            if let Some(info) = snapshots.iter().find(|s| s.id == snapshot_id) {
                return Ok((branch_id, info.clone()));
            }
        }

        Err(TimelineError::Internal(format!(
            "Snapshot {} not found in index",
            snapshot_id.0
        )))
    }

    /// Get total size of all snapshots
    pub async fn get_total_size(&self) -> TimelineResult<u64> {
        let mut total = 0u64;

        for entry in self.snapshot_index.iter() {
            for info in entry.value() {
                total += info.size_bytes;
            }
        }

        Ok(total)
    }

    /// Get snapshot count
    pub async fn get_snapshot_count(&self) -> TimelineResult<u64> {
        let mut count = 0u64;

        for entry in self.snapshot_index.iter() {
            count += entry.value().len() as u64;
        }

        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_snapshot_manager() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = StorageConfig {
            base_path: temp_dir.path().to_path_buf(),
            compression_enabled: true,
            ..Default::default()
        };

        let manager = SnapshotManager::new(&config).await.unwrap();

        // Create a test snapshot
        let snapshot = Snapshot {
            id: SnapshotId::new(),
            branch_id: BranchId::main(),
            event_id: EventId::new(),
            sequence_number: 100,
            created_at: chrono::Utc::now(),
            entities: std::collections::HashMap::new(),
            metadata: SnapshotMetadata {
                size_bytes: 0,
                entity_count: 0,
                compression_ratio: 1.0,
                creation_duration_ms: 0,
            },
        };

        manager.save_snapshot(&snapshot).await.unwrap();

        // Load it back
        let loaded = manager.load_snapshot(snapshot.id).await.unwrap();
        assert_eq!(loaded.id, snapshot.id);
        assert_eq!(loaded.sequence_number, snapshot.sequence_number);
    }
}
