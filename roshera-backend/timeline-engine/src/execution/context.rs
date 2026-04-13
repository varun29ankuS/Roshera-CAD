//! Execution context for operations

use crate::{BranchId, EntityId, EntityType, TimelineError, TimelineResult};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// Context for operation execution
pub struct ExecutionContext {
    /// Current branch
    pub branch_id: BranchId,

    /// Entity state store
    pub entity_store: Arc<EntityStateStore>,

    /// Temporary entities created during execution
    temp_entities: DashMap<EntityId, EntityState>,

    /// Performance counters
    geometry_ops: AtomicU64,
    memory_allocated: AtomicU64,
}

/// Store for entity states
pub struct EntityStateStore {
    /// Entity states by ID
    entities: Arc<DashMap<EntityId, EntityState>>,

    /// Entity IDs by type
    entities_by_type: Arc<DashMap<EntityType, DashMap<EntityId, ()>>>,
}

/// State of an entity
#[derive(Debug, Clone)]
pub struct EntityState {
    /// Entity ID
    pub id: EntityId,

    /// Entity type
    pub entity_type: EntityType,

    /// Serialized geometry data
    pub geometry_data: Vec<u8>,

    /// Entity properties
    pub properties: serde_json::Value,

    /// Whether this entity is deleted
    pub is_deleted: bool,
}

impl ExecutionContext {
    /// Create a new execution context
    pub fn new(branch_id: BranchId, entity_store: Arc<EntityStateStore>) -> Self {
        Self {
            branch_id,
            entity_store,
            temp_entities: DashMap::new(),
            geometry_ops: AtomicU64::new(0),
            memory_allocated: AtomicU64::new(0),
        }
    }

    /// Get an entity by ID
    pub fn get_entity(&self, id: EntityId) -> TimelineResult<EntityState> {
        // Check temp entities first
        if let Some(entity) = self.temp_entities.get(&id) {
            return Ok(entity.clone());
        }

        // Then check persistent store
        self.entity_store.get_entity(id)
    }

    /// Check if an entity exists
    pub fn entity_exists(&self, id: EntityId) -> bool {
        self.temp_entities.contains_key(&id) || self.entity_store.entity_exists(id)
    }

    /// Add a temporary entity
    pub fn add_temp_entity(&self, entity: EntityState) -> TimelineResult<()> {
        if self.entity_exists(entity.id) {
            return Err(TimelineError::ValidationError(format!(
                "Entity {} already exists",
                entity.id
            )));
        }

        let size = entity.geometry_data.len() as u64;
        self.memory_allocated.fetch_add(size, Ordering::Relaxed);

        self.temp_entities.insert(entity.id, entity);
        Ok(())
    }

    /// Update an entity
    pub fn update_entity(&self, id: EntityId, entity: EntityState) -> TimelineResult<()> {
        if !self.entity_exists(id) {
            return Err(TimelineError::EntityNotFound(id));
        }

        // Update in temp if it exists there
        if self.temp_entities.contains_key(&id) {
            self.temp_entities.insert(id, entity);
        } else {
            // Otherwise mark for update in persistent store
            self.temp_entities.insert(id, entity);
        }

        Ok(())
    }

    /// Mark an entity as deleted
    pub fn delete_entity(&self, id: EntityId) -> TimelineResult<()> {
        if !self.entity_exists(id) {
            return Err(TimelineError::EntityNotFound(id));
        }

        // If in temp, just remove it
        if let Some((_, entity)) = self.temp_entities.remove(&id) {
            let size = entity.geometry_data.len() as u64;
            self.memory_allocated.fetch_sub(size, Ordering::Relaxed);
        } else {
            // Otherwise mark as deleted
            let mut entity = self.entity_store.get_entity(id)?;
            entity.is_deleted = true;
            self.temp_entities.insert(id, entity);
        }

        Ok(())
    }

    /// Get entities of a specific type
    pub fn get_entities_by_type(&self, entity_type: EntityType) -> Vec<EntityState> {
        let mut entities = Vec::new();

        // Get from temp entities
        for entry in self.temp_entities.iter() {
            if entry.value().entity_type == entity_type && !entry.value().is_deleted {
                entities.push(entry.value().clone());
            }
        }

        // Get from persistent store (excluding those in temp)
        if let Some(type_entities) = self.entity_store.entities_by_type.get(&entity_type) {
            for entry in type_entities.iter() {
                let id = *entry.key();
                if !self.temp_entities.contains_key(&id) {
                    if let Ok(entity) = self.entity_store.get_entity(id) {
                        if !entity.is_deleted {
                            entities.push(entity);
                        }
                    }
                }
            }
        }

        entities
    }

    /// Increment geometry operation count
    pub fn increment_geometry_ops(&self) {
        self.geometry_ops.fetch_add(1, Ordering::Relaxed);
    }

    /// Get geometry operation count
    pub fn get_geometry_op_count(&self) -> u64 {
        self.geometry_ops.load(Ordering::Relaxed)
    }

    /// Get memory allocated
    pub fn get_memory_allocated(&self) -> u64 {
        self.memory_allocated.load(Ordering::Relaxed)
    }

    /// Commit temporary entities to the store
    pub fn commit(self) -> TimelineResult<Vec<EntityId>> {
        let mut committed_ids = Vec::new();

        for (id, entity) in self.temp_entities {
            self.entity_store.update_entity(entity)?;
            committed_ids.push(id);
        }

        Ok(committed_ids)
    }
}

impl EntityStateStore {
    /// Create a new entity state store
    pub fn new() -> Self {
        Self {
            entities: Arc::new(DashMap::new()),
            entities_by_type: Arc::new(DashMap::new()),
        }
    }

    /// Get an entity by ID
    pub fn get_entity(&self, id: EntityId) -> TimelineResult<EntityState> {
        self.entities
            .get(&id)
            .map(|entry| entry.clone())
            .ok_or(TimelineError::EntityNotFound(id))
    }

    /// Check if an entity exists
    pub fn entity_exists(&self, id: EntityId) -> bool {
        self.entities.contains_key(&id)
    }

    /// Add a new entity
    pub fn add_entity(&self, entity: EntityState) -> TimelineResult<()> {
        if self.entity_exists(entity.id) {
            return Err(TimelineError::ValidationError(format!(
                "Entity {} already exists",
                entity.id
            )));
        }
        self.update_entity(entity)
    }

    /// Update or insert an entity
    pub fn update_entity(&self, entity: EntityState) -> TimelineResult<()> {
        let id = entity.id;
        let entity_type = entity.entity_type;
        let is_deleted = entity.is_deleted;

        // Update main store
        self.entities.insert(id, entity);

        // Update type index
        if is_deleted {
            // Remove from type index
            if let Some(type_entities) = self.entities_by_type.get(&entity_type) {
                type_entities.remove(&id);
            }
        } else {
            // Add to type index
            self.entities_by_type
                .entry(entity_type)
                .or_insert_with(DashMap::new)
                .insert(id, ());
        }

        Ok(())
    }

    /// Remove an entity (mark as deleted)
    pub fn remove_entity(&self, id: EntityId) -> TimelineResult<()> {
        if let Some(mut entity) = self.entities.get_mut(&id) {
            entity.is_deleted = true;
            // Remove from type index
            if let Some(type_entities) = self.entities_by_type.get(&entity.entity_type) {
                type_entities.remove(&id);
            }
            Ok(())
        } else {
            Err(TimelineError::EntityNotFound(id))
        }
    }

    /// Get entities by type
    pub fn get_entities_by_type(&self, entity_type: EntityType) -> Vec<EntityId> {
        self.entities_by_type
            .get(&entity_type)
            .map(|type_entities| type_entities.iter().map(|entry| *entry.key()).collect())
            .unwrap_or_default()
    }

    /// Get all entities
    pub fn get_all_entities(&self) -> Vec<EntityState> {
        self.entities
            .iter()
            .filter(|entry| !entry.value().is_deleted)
            .map(|entry| entry.value().clone())
            .collect()
    }

    /// Clear all entities
    pub fn clear(&self) {
        self.entities.clear();
        self.entities_by_type.clear();
    }
}

impl Default for EntityStateStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_state_store() {
        let store = EntityStateStore::new();

        let entity = EntityState {
            id: EntityId::new(),
            entity_type: EntityType::Solid,
            geometry_data: vec![1, 2, 3],
            properties: serde_json::json!({}),
            is_deleted: false,
        };

        store.update_entity(entity.clone()).unwrap();

        let retrieved = store.get_entity(entity.id).unwrap();
        assert_eq!(retrieved.id, entity.id);
        assert_eq!(retrieved.entity_type, entity.entity_type);
    }

    #[test]
    fn test_execution_context() {
        let store = Arc::new(EntityStateStore::new());
        let context = ExecutionContext::new(BranchId::main(), store);

        let entity = EntityState {
            id: EntityId::new(),
            entity_type: EntityType::Sketch,
            geometry_data: vec![4, 5, 6],
            properties: serde_json::json!({}),
            is_deleted: false,
        };

        context.add_temp_entity(entity.clone()).unwrap();
        assert!(context.entity_exists(entity.id));

        let retrieved = context.get_entity(entity.id).unwrap();
        assert_eq!(retrieved.id, entity.id);
    }
}
