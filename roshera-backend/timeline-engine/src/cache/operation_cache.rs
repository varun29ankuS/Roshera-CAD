//! Cache for operation results

use super::{CacheStats, CachedItem};
use crate::{BranchId, EntityId, EntityType, OperationOutputs};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use lru::LruCache;
use parking_lot::RwLock;
use std::sync::Arc;

/// Cache for operation execution results
pub struct OperationCache {
    /// LRU cache for each branch
    branch_caches: DashMap<BranchId, Arc<RwLock<LruCache<String, CachedItem<OperationOutputs>>>>>,

    /// Maximum items per branch cache
    max_items: usize,

    /// TTL for cached items
    ttl_seconds: u64,

    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
}

impl OperationCache {
    /// Create a new operation cache
    pub fn new(max_items: usize, ttl_seconds: u64) -> Self {
        Self {
            branch_caches: DashMap::new(),
            max_items,
            ttl_seconds,
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Get cached operation result
    pub fn get(&self, branch_id: BranchId, key: &str) -> Option<OperationOutputs> {
        let branch_cache = self.branch_caches.get(&branch_id)?;
        let mut cache = branch_cache.write();

        if let Some(item) = cache.get_mut(key) {
            // Check if expired
            let age = Utc::now() - item.cached_at;
            if age.num_seconds() as u64 > self.ttl_seconds {
                cache.pop(key);
                self.update_stats(|s| s.misses += 1);
                return None;
            }

            // Update access info
            item.access_count += 1;
            item.last_accessed = Utc::now();

            self.update_stats(|s| s.hits += 1);
            Some(item.value.clone())
        } else {
            self.update_stats(|s| s.misses += 1);
            None
        }
    }

    /// Put operation result in cache
    pub fn put(&self, branch_id: BranchId, key: String, outputs: OperationOutputs) {
        let size_bytes = estimate_operation_size(&outputs);

        let item = CachedItem {
            value: outputs,
            cached_at: Utc::now(),
            size_bytes,
            access_count: 0,
            last_accessed: Utc::now(),
        };

        // Get or create branch cache
        let branch_cache = self.branch_caches.entry(branch_id).or_insert_with(|| {
            Arc::new(RwLock::new(LruCache::new(
                std::num::NonZeroUsize::new(self.max_items).unwrap(),
            )))
        });

        let mut cache = branch_cache.write();

        // Check if we're replacing an existing item
        let replacing = cache.contains(&key);

        // Insert the new item
        if let Some((_evicted_key, evicted_item)) = cache.push(key, item) {
            self.update_stats(|s| {
                s.evictions += 1;
                s.size_bytes = s.size_bytes.saturating_sub(evicted_item.size_bytes);
            });
        }

        self.update_stats(|s| {
            if !replacing {
                s.item_count += 1;
            }
            s.size_bytes += size_bytes;
        });
    }

    /// Invalidate cache entries for specific entities
    pub fn invalidate_entities(&self, branch_id: BranchId, entities: &[EntityId]) {
        if let Some(branch_cache) = self.branch_caches.get(&branch_id) {
            let mut cache = branch_cache.write();
            let mut keys_to_remove = Vec::new();

            // Find all keys that involve these entities
            // This is a simplified approach - in production would use better indexing
            for (key, item) in cache.iter() {
                if involves_entities(&item.value, entities) {
                    keys_to_remove.push(key.clone());
                }
            }

            // Remove the keys
            for key in keys_to_remove {
                if let Some(item) = cache.pop(&key) {
                    self.update_stats(|s| {
                        s.item_count = s.item_count.saturating_sub(1);
                        s.size_bytes = s.size_bytes.saturating_sub(item.size_bytes);
                    });
                }
            }
        }
    }

    /// Clear cache for a branch
    pub fn clear_branch(&self, branch_id: BranchId) {
        if let Some((_, branch_cache)) = self.branch_caches.remove(&branch_id) {
            let cache = branch_cache.read();
            let items = cache.len();
            let size: usize = cache.iter().map(|(_, item)| item.size_bytes).sum();

            drop(cache);

            self.update_stats(|s| {
                s.item_count = s.item_count.saturating_sub(items);
                s.size_bytes = s.size_bytes.saturating_sub(size);
            });
        }
    }

    /// Clear all caches
    pub fn clear(&self) {
        self.branch_caches.clear();
        *self.stats.write() = CacheStats::default();
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> CacheStats {
        self.stats.read().clone()
    }

    /// Get memory usage
    pub fn memory_usage(&self) -> usize {
        self.stats.read().size_bytes
    }

    /// Evict oldest items across all branches
    pub fn evict_oldest(&self) {
        // Find the oldest item across all branches
        let mut oldest: Option<(BranchId, String, DateTime<Utc>)> = None;

        for entry in self.branch_caches.iter() {
            let branch_id = *entry.key();
            let cache = entry.value().read();

            for (key, item) in cache.iter() {
                if oldest.is_none() || item.last_accessed < oldest.as_ref().unwrap().2 {
                    oldest = Some((branch_id, key.clone(), item.last_accessed));
                }
            }
        }

        // Evict the oldest item
        if let Some((branch_id, key, _)) = oldest {
            if let Some(branch_cache) = self.branch_caches.get(&branch_id) {
                let mut cache = branch_cache.write();
                if let Some(item) = cache.pop(&key) {
                    self.update_stats(|s| {
                        s.evictions += 1;
                        s.item_count = s.item_count.saturating_sub(1);
                        s.size_bytes = s.size_bytes.saturating_sub(item.size_bytes);
                    });
                }
            }
        }
    }

    /// Update statistics atomically
    fn update_stats<F>(&self, f: F)
    where
        F: FnOnce(&mut CacheStats),
    {
        let mut stats = self.stats.write();
        f(&mut stats);
    }
}

/// Estimate size of operation outputs
fn estimate_operation_size(outputs: &OperationOutputs) -> usize {
    // Base size
    let mut size = std::mem::size_of::<OperationOutputs>();

    // Add size of created entities
    size += outputs.created.len()
        * (std::mem::size_of::<EntityId>() + std::mem::size_of::<EntityType>() + 32); // 32 bytes for optional name

    // Add size of modified entities
    size += outputs.modified.len() * std::mem::size_of::<EntityId>();

    // Add size of deleted entities
    size += outputs.deleted.len() * std::mem::size_of::<EntityId>();

    // Add approximate size of side effects
    size += outputs.side_effects.len() * 64; // Rough estimate for side effects

    size
}

/// Check if operation outputs involve specific entities
fn involves_entities(outputs: &OperationOutputs, entities: &[EntityId]) -> bool {
    for entity in entities {
        // Check created entities
        if outputs.created.iter().any(|e| e.id == *entity) {
            return true;
        }

        // Check modified and deleted entities
        if outputs.modified.contains(entity) || outputs.deleted.contains(entity) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_operation_cache_basic() {
        let cache = OperationCache::new(100, 3600);
        let branch_id = BranchId::main();
        let key = "test_operation";

        // Test miss
        assert!(cache.get(branch_id, key).is_none());

        // Test put and hit
        let outputs = OperationOutputs {
            created: vec![crate::CreatedEntity {
                id: EntityId::new(),
                entity_type: crate::EntityType::Solid,
                name: None,
            }],
            modified: vec![],
            deleted: vec![],
            side_effects: vec![],
        };

        cache.put(branch_id, key.to_string(), outputs.clone());

        let cached = cache.get(branch_id, key);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().created.len(), outputs.created.len());

        // Check stats
        let stats = cache.get_stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.item_count, 1);
    }

    #[test]
    fn test_cache_expiration() {
        let cache = OperationCache::new(100, 0); // 0 second TTL
        let branch_id = BranchId::main();
        let key = "test_operation";

        let outputs = OperationOutputs {
            created: vec![],
            modified: vec![],
            deleted: vec![],
            side_effects: vec![],
        };

        cache.put(branch_id, key.to_string(), outputs);

        // Should be expired immediately
        assert!(cache.get(branch_id, key).is_none());
    }

    #[test]
    fn test_cache_invalidation() {
        let cache = OperationCache::new(100, 3600);
        let branch_id = BranchId::main();
        let entity_id = EntityId::new();

        let outputs = OperationOutputs {
            created: vec![crate::CreatedEntity {
                id: entity_id,
                entity_type: crate::EntityType::Solid,
                name: None,
            }],
            modified: vec![],
            deleted: vec![],
            side_effects: vec![],
        };

        cache.put(branch_id, "op1".to_string(), outputs.clone());
        cache.put(branch_id, "op2".to_string(), outputs);

        // Both should be cached
        assert!(cache.get(branch_id, "op1").is_some());
        assert!(cache.get(branch_id, "op2").is_some());

        // Invalidate entities
        cache.invalidate_entities(branch_id, &[entity_id]);

        // Both should be invalidated
        assert!(cache.get(branch_id, "op1").is_none());
        assert!(cache.get(branch_id, "op2").is_none());
    }
}
