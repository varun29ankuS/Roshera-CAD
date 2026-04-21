//! Storage index for fast event lookups

use super::event_log::EventLocation;
use crate::{
    BranchId, EntityId, EventId, StorageConfig, TimelineError, TimelineEvent, TimelineResult,
};
use dashmap::DashMap;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use tokio::fs;

/// Index for fast event lookups
pub struct StorageIndex {
    /// Event ID to location mapping
    event_locations: Arc<DashMap<EventId, EventLocation>>,

    /// Branch to event mapping (sequence number -> event ID)
    branch_events: Arc<DashMap<BranchId, DashMap<u64, EventId>>>,

    /// Entity to events that created it
    entity_creators: Arc<DashMap<EntityId, Vec<EventId>>>,

    /// Entity to events that modified it
    entity_modifiers: Arc<DashMap<EntityId, Vec<EventId>>>,

    /// Total event count
    event_count: Arc<AtomicU64>,

    /// Index file path
    index_path: std::path::PathBuf,
}

impl StorageIndex {
    /// Create a new storage index
    pub async fn new(config: &StorageConfig) -> TimelineResult<Self> {
        let index_path = config.base_path.join("index");
        tokio::fs::create_dir_all(&index_path)
            .await
            .map_err(TimelineError::StorageError)?;

        let index = Self {
            event_locations: Arc::new(DashMap::new()),
            branch_events: Arc::new(DashMap::new()),
            entity_creators: Arc::new(DashMap::new()),
            entity_modifiers: Arc::new(DashMap::new()),
            event_count: Arc::new(AtomicU64::new(0)),
            index_path,
        };

        // Load existing index if available
        index.load_from_disk().await?;

        Ok(index)
    }

    /// Index an event with location information
    pub async fn index_event_with_location(
        &self,
        event: &TimelineEvent,
        segment: u32,
        offset: u64,
        size: u32,
    ) -> TimelineResult<()> {
        // Store event location
        let location = EventLocation {
            segment,
            offset,
            size,
        };

        self.event_locations.insert(event.id, location);

        // Index by branch
        self.branch_events
            .entry(event.metadata.branch_id)
            .or_insert_with(DashMap::new)
            .insert(event.sequence_number, event.id);

        // Index by entities
        for created in &event.outputs.created {
            self.entity_creators
                .entry(created.id)
                .or_insert_with(Vec::new)
                .push(event.id);
        }

        for &modified in &event.outputs.modified {
            self.entity_modifiers
                .entry(modified)
                .or_insert_with(Vec::new)
                .push(event.id);
        }

        // Update count
        self.event_count.fetch_add(1, Ordering::Relaxed);

        // Periodically persist index
        if self.event_count.load(Ordering::Relaxed) % 100 == 0 {
            self.persist_to_disk().await?;
        }

        Ok(())
    }

    /// Index an event (convenience method without location)
    pub async fn index_event(&self, event: &TimelineEvent) -> TimelineResult<()> {
        // Use default location for in-memory operations
        self.index_event_with_location(event, 0, 0, 0).await
    }

    /// Get event location by ID
    pub async fn get_event_location(&self, event_id: EventId) -> TimelineResult<EventLocation> {
        self.event_locations
            .get(&event_id)
            .map(|entry| *entry)
            .ok_or(TimelineError::EventNotFound(event_id))
    }

    /// Get events for a branch in a range
    pub async fn get_branch_events(
        &self,
        branch_id: BranchId,
        start: u64,
        end: u64,
    ) -> TimelineResult<Vec<EventLocation>> {
        let branch_events = self
            .branch_events
            .get(&branch_id)
            .ok_or(TimelineError::BranchNotFound(branch_id))?;

        let mut locations = Vec::new();

        for seq in start..=end {
            if let Some(event_id) = branch_events.get(&seq) {
                if let Some(location) = self.event_locations.get(&event_id) {
                    locations.push(*location);
                }
            }
        }

        Ok(locations)
    }

    /// Get events that created an entity
    pub async fn get_entity_creators(&self, entity_id: EntityId) -> Vec<EventId> {
        self.entity_creators
            .get(&entity_id)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Get events that modified an entity
    pub async fn get_entity_modifiers(&self, entity_id: EntityId) -> Vec<EventId> {
        self.entity_modifiers
            .get(&entity_id)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Get total event count
    pub async fn get_event_count(&self) -> TimelineResult<u64> {
        Ok(self.event_count.load(Ordering::Relaxed))
    }

    /// Get index size on disk
    pub async fn get_size(&self) -> TimelineResult<u64> {
        let mut total_size = 0u64;

        let mut entries = tokio::fs::read_dir(&self.index_path)
            .await
            .map_err(TimelineError::StorageError)?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            let metadata = entry
                .metadata()
                .await
                .map_err(TimelineError::StorageError)?;

            if metadata.is_file() {
                total_size += metadata.len();
            }
        }

        Ok(total_size)
    }

    /// Persist index to disk
    async fn persist_to_disk(&self) -> TimelineResult<()> {
        // Create index snapshot
        let snapshot = IndexSnapshot {
            event_count: self.event_count.load(Ordering::Relaxed),
            timestamp: chrono::Utc::now(),
        };

        // Save main index file
        let index_file = self.index_path.join("index.dat");
        let data = bincode::serialize(&snapshot)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        fs::write(&index_file, data)
            .await
            .map_err(TimelineError::StorageError)?;

        // Save event locations
        let locations_file = self.index_path.join("locations.dat");
        let locations: Vec<(EventId, EventLocation)> = self
            .event_locations
            .iter()
            .map(|entry| (*entry.key(), *entry.value()))
            .collect();

        let data = bincode::serialize(&locations)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        fs::write(&locations_file, data)
            .await
            .map_err(TimelineError::StorageError)?;

        // Save branch events index
        let branch_file = self.index_path.join("branch_events.dat");
        let branch_data: Vec<(BranchId, Vec<(u64, EventId)>)> = self
            .branch_events
            .iter()
            .map(|entry| {
                let branch_id = *entry.key();
                let events: Vec<(u64, EventId)> = entry
                    .value()
                    .iter()
                    .map(|e| (*e.key(), *e.value()))
                    .collect();
                (branch_id, events)
            })
            .collect();

        let data = bincode::serialize(&branch_data)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;
        fs::write(&branch_file, data)
            .await
            .map_err(TimelineError::StorageError)?;

        // Save entity creators index
        let entity_file = self.index_path.join("entity_creators.dat");
        let entity_data: Vec<(EntityId, Vec<EventId>)> = self
            .entity_creators
            .iter()
            .map(|entry| (*entry.key(), entry.value().clone()))
            .collect();

        let data = bincode::serialize(&entity_data)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;
        fs::write(&entity_file, data)
            .await
            .map_err(TimelineError::StorageError)?;

        Ok(())
    }

    /// Load index from disk
    async fn load_from_disk(&self) -> TimelineResult<()> {
        let index_file = self.index_path.join("index.dat");

        if !index_file.exists() {
            return Ok(());
        }

        // Load main index
        let data = fs::read(&index_file)
            .await
            .map_err(TimelineError::StorageError)?;

        let snapshot: IndexSnapshot = bincode::deserialize(&data)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        self.event_count
            .store(snapshot.event_count, Ordering::Relaxed);

        // Load event locations
        let locations_file = self.index_path.join("locations.dat");
        if locations_file.exists() {
            let data = fs::read(&locations_file)
                .await
                .map_err(TimelineError::StorageError)?;

            let locations: Vec<(EventId, EventLocation)> = bincode::deserialize(&data)
                .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

            for (event_id, location) in locations {
                self.event_locations.insert(event_id, location);
            }
        }

        // Load branch events index
        let branch_file = self.index_path.join("branch_events.dat");
        if branch_file.exists() {
            let data = fs::read(&branch_file)
                .await
                .map_err(TimelineError::StorageError)?;

            let branch_data: Vec<(BranchId, Vec<(u64, EventId)>)> = bincode::deserialize(&data)
                .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

            for (branch_id, events) in branch_data {
                let branch_map = self
                    .branch_events
                    .entry(branch_id)
                    .or_insert_with(DashMap::new);
                for (seq_num, event_id) in events {
                    branch_map.insert(seq_num, event_id);
                }
            }
        }

        // Load entity creators index
        let entity_file = self.index_path.join("entity_creators.dat");
        if entity_file.exists() {
            let data = fs::read(&entity_file)
                .await
                .map_err(TimelineError::StorageError)?;

            let entity_data: Vec<(EntityId, Vec<EventId>)> = bincode::deserialize(&data)
                .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

            for (entity_id, event_ids) in entity_data {
                self.entity_creators.insert(entity_id, event_ids);
            }
        }

        tracing::info!("Loaded index with {} events", snapshot.event_count);

        Ok(())
    }

    /// Rebuild index from event log
    pub async fn rebuild_from_log(&self) -> TimelineResult<()> {
        // Clear existing indexes
        self.event_locations.clear();
        self.branch_events.clear();
        self.entity_creators.clear();
        self.entity_modifiers.clear();

        // Scan all segment files in the storage directory
        // Use the parent directory of the index path as the base path
        let base_path = self
            .index_path
            .parent()
            .unwrap_or(std::path::Path::new("."));
        let mut entries = tokio::fs::read_dir(&base_path)
            .await
            .map_err(TimelineError::StorageError)?;

        let mut segment_files = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            let file_name = entry.file_name();
            let file_name_str = file_name.to_string_lossy();

            if file_name_str.starts_with("segment_") && file_name_str.ends_with(".log") {
                segment_files.push(entry.path());
            }
        }

        // Sort segment files by name to process in order
        segment_files.sort();

        // Process each segment file
        for (_segment_idx, segment_path) in segment_files.iter().enumerate() {
            let file_data = tokio::fs::read(segment_path)
                .await
                .map_err(TimelineError::StorageError)?;

            let mut offset = 0;
            while offset < file_data.len() {
                // Try to deserialize an event from the current position
                if let Ok(event) = bincode::deserialize::<TimelineEvent>(&file_data[offset..]) {
                    // Index the event
                    self.index_event(&event).await?;

                    // Move to next event (estimate size)
                    let event_size = bincode::serialized_size(&event).unwrap_or(1024) as usize;
                    offset += event_size;
                } else {
                    // Skip to next potential event boundary
                    offset += 1;
                }
            }
        }

        tracing::info!("Rebuilt index with {} events", self.event_locations.len());
        Ok(())
    }

    /// Verify index integrity
    pub async fn verify_integrity(&self) -> TimelineResult<bool> {
        let mut is_valid = true;

        // Check event index consistency
        for entry in self.event_locations.iter() {
            let event_id = entry.key();
            let location = entry.value();

            // Verify the event exists at the specified location. Fall back
            // to the current directory if `index_path` has no parent (e.g.
            // a bare filename); this is exceedingly rare in practice.
            let parent = self
                .index_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            let segment_path = parent.join(format!("segment_{:06}.log", location.segment));
            if !segment_path.exists() {
                tracing::warn!("Missing segment file for event {:?}", event_id);
                is_valid = false;
            }
        }

        // Check branch index consistency
        for entry in self.branch_events.iter() {
            let branch_id = entry.key();
            let event_ids = entry.value();

            // Verify all referenced events exist in event index
            for inner_entry in event_ids.iter() {
                let event_id = inner_entry.value();
                if !self.event_locations.contains_key(event_id) {
                    tracing::warn!(
                        "Branch {:?} references missing event {:?}",
                        branch_id,
                        event_id
                    );
                    is_valid = false;
                }
            }
        }

        // Check entity index consistency
        for entry in self.entity_creators.iter() {
            let entity_id = entry.key();
            let event_ids = entry.value();

            // Verify all referenced events exist
            for event_id in event_ids {
                if !self.event_locations.contains_key(event_id) {
                    tracing::warn!(
                        "Entity {:?} references missing event {:?}",
                        entity_id,
                        event_id
                    );
                    is_valid = false;
                }
            }
        }

        Ok(is_valid)
    }
}

/// Index snapshot for persistence
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct IndexSnapshot {
    event_count: u64,
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, EventMetadata, Operation, OperationInputs, OperationOutputs};

    #[tokio::test]
    async fn test_index_operations() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = StorageConfig {
            base_path: temp_dir.path().to_path_buf(),
            ..Default::default()
        };

        let index = StorageIndex::new(&config).await.unwrap();

        // Create test event
        let event = TimelineEvent {
            id: EventId::new(),
            sequence_number: 1,
            timestamp: chrono::Utc::now(),
            author: Author::System,
            operation: Operation::CreatePrimitive {
                primitive_type: crate::PrimitiveType::Box,
                parameters: serde_json::json!({}),
            },
            inputs: OperationInputs {
                required_entities: vec![],
                optional_entities: vec![],
                parameters: serde_json::Value::Null,
            },
            outputs: OperationOutputs {
                created: vec![],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            },
            metadata: EventMetadata {
                description: None,
                branch_id: BranchId::main(),
                tags: vec![],
                properties: std::collections::HashMap::new(),
            },
        };

        // Index the event
        index.index_event(&event).await.unwrap();

        // Verify it was indexed
        assert_eq!(index.get_event_count().await.unwrap(), 1);

        // Verify we can look it up
        let location = index.get_event_location(event.id).await.unwrap();
        assert_eq!(location.segment, 0); // Default value for now
    }
}
