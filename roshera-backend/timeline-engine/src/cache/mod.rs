//! Caching layer for timeline operations and computed results

use crate::{
    BranchId, EntityId, Operation, TimelineResult,
};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use std::sync::Arc;

mod dependency_cache;
mod operation_cache;
mod tessellation_cache;

pub use dependency_cache::DependencyCache;
pub use operation_cache::OperationCache;
pub use tessellation_cache::TessellationCache;

/// Main cache manager for the timeline system
pub struct CacheManager {
    /// Operation result cache
    operation_cache: Arc<OperationCache>,

    /// Tessellation cache for display meshes
    tessellation_cache: Arc<TessellationCache>,

    /// Dependency relationship cache
    dependency_cache: Arc<DependencyCache>,

    /// Configuration
    config: CacheConfig,

    /// Cache statistics
    stats: Arc<DashMap<String, CacheStats>>,
}

/// Cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Maximum memory usage in bytes
    pub max_memory_bytes: usize,

    /// Maximum items per cache
    pub max_items_per_cache: usize,

    /// TTL for cached items in seconds
    pub ttl_seconds: u64,

    /// Enable compression for large items
    pub enable_compression: bool,

    /// Compression threshold in bytes
    pub compression_threshold: usize,
}

/// Statistics for a cache
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Total hits
    pub hits: u64,

    /// Total misses
    pub misses: u64,

    /// Current size in bytes
    pub size_bytes: usize,

    /// Number of items
    pub item_count: usize,

    /// Number of evictions
    pub evictions: u64,
}

/// Cached item wrapper
#[derive(Debug, Clone)]
pub struct CachedItem<T> {
    /// The cached value
    pub value: T,

    /// When this was cached
    pub cached_at: DateTime<Utc>,

    /// Size in bytes
    pub size_bytes: usize,

    /// Number of times accessed
    pub access_count: u64,

    /// Last access time
    pub last_accessed: DateTime<Utc>,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_memory_bytes: 512 * 1024 * 1024, // 512MB
            max_items_per_cache: 10000,
            ttl_seconds: 3600, // 1 hour
            enable_compression: true,
            compression_threshold: 10 * 1024, // 10KB
        }
    }
}

impl CacheManager {
    /// Create a new cache manager
    pub fn new(config: CacheConfig) -> Self {
        let operation_cache = Arc::new(OperationCache::new(
            config.max_items_per_cache,
            config.ttl_seconds,
        ));

        let tessellation_cache = Arc::new(TessellationCache::new(
            config.max_items_per_cache / 10, // Tessellations are larger
            config.ttl_seconds,
        ));

        let dependency_cache = Arc::new(DependencyCache::new(config.max_items_per_cache));

        Self {
            operation_cache,
            tessellation_cache,
            dependency_cache,
            config,
            stats: Arc::new(DashMap::new()),
        }
    }

    /// Get operation cache
    pub fn operation_cache(&self) -> &Arc<OperationCache> {
        &self.operation_cache
    }

    /// Get tessellation cache
    pub fn tessellation_cache(&self) -> &Arc<TessellationCache> {
        &self.tessellation_cache
    }

    /// Get dependency cache
    pub fn dependency_cache(&self) -> &Arc<DependencyCache> {
        &self.dependency_cache
    }

    /// Clear all caches
    pub fn clear_all(&self) {
        self.operation_cache.clear();
        self.tessellation_cache.clear();
        self.dependency_cache.clear();
        self.stats.clear();
    }

    /// Clear caches for a specific branch
    pub fn clear_branch(&self, branch_id: BranchId) {
        self.operation_cache.clear_branch(branch_id);
        self.tessellation_cache.clear_branch(branch_id);
        // Dependencies are global, not branch-specific
    }

    /// Get cache statistics
    pub fn get_stats(&self) -> HashMap<String, CacheStats> {
        let mut stats = HashMap::new();

        stats.insert(
            "operation_cache".to_string(),
            self.operation_cache.get_stats(),
        );

        stats.insert(
            "tessellation_cache".to_string(),
            self.tessellation_cache.get_stats(),
        );

        stats.insert(
            "dependency_cache".to_string(),
            self.dependency_cache.get_stats(),
        );

        stats
    }

    /// Get total memory usage
    pub fn total_memory_usage(&self) -> usize {
        self.operation_cache.memory_usage()
            + self.tessellation_cache.memory_usage()
            + self.dependency_cache.memory_usage()
    }

    /// Check if memory limit is exceeded
    pub fn is_memory_limit_exceeded(&self) -> bool {
        self.total_memory_usage() > self.config.max_memory_bytes
    }

    /// Evict items if memory limit exceeded
    pub fn evict_if_needed(&self) {
        if self.is_memory_limit_exceeded() {
            // Evict from largest cache first
            let mut caches: Vec<(&str, usize)> = vec![
                ("operation", self.operation_cache.memory_usage()),
                ("tessellation", self.tessellation_cache.memory_usage()),
                ("dependency", self.dependency_cache.memory_usage()),
            ];

            caches.sort_by_key(|(_, size)| *size);
            caches.reverse();

            for (cache_name, _) in caches {
                match cache_name {
                    "operation" => self.operation_cache.evict_oldest(),
                    "tessellation" => self.tessellation_cache.evict_oldest(),
                    "dependency" => self.dependency_cache.evict_oldest(),
                    _ => {}
                }

                if !self.is_memory_limit_exceeded() {
                    break;
                }
            }
        }
    }
}

/// Cache key generation utilities
pub mod cache_key {
    use super::*;
    use sha2::{Digest, Sha256};

    /// Generate cache key for an operation
    pub fn operation_key(
        branch_id: BranchId,
        operation: &Operation,
        inputs: &serde_json::Value,
    ) -> String {
        let mut hasher = Sha256::new();
        hasher.update(branch_id.to_string().as_bytes());
        hasher.update(format!("{:?}", operation).as_bytes());
        hasher.update(inputs.to_string().as_bytes());

        format!("{:x}", hasher.finalize())
    }

    /// Generate cache key for tessellation
    pub fn tessellation_key(entity_id: EntityId, quality: f64, format: &str) -> String {
        format!("{}_{}_{}", entity_id, quality, format)
    }

    /// Generate cache key for dependencies
    pub fn dependency_key(entity_id: EntityId) -> String {
        entity_id.to_string()
    }
}

/// Cache warmup utilities
pub mod cache_warmup {
    use super::*;

    /// Warmup operation cache with common operations
    pub async fn warmup_operations(
        _cache: &Arc<OperationCache>,
        common_ops: Vec<(BranchId, Operation, serde_json::Value)>,
    ) -> TimelineResult<()> {
        for (branch_id, op, inputs) in common_ops {
            // Pre-compute and cache
            let key = cache_key::operation_key(branch_id, &op, &inputs);
            // In real implementation, would execute operation and cache result
            tracing::debug!("Warming up operation cache with key: {}", key);
        }

        Ok(())
    }

    /// Analyze access patterns and pre-warm cache
    pub async fn analyze_and_warmup(
        _cache_manager: &CacheManager,
        access_log: Vec<AccessLogEntry>,
    ) -> TimelineResult<()> {
        // Analyze most frequently accessed items
        let mut frequency_map = HashMap::new();

        for entry in access_log {
            *frequency_map.entry(entry.cache_key).or_insert(0) += 1;
        }

        // Sort by frequency
        let mut frequent_items: Vec<_> = frequency_map.into_iter().collect();
        frequent_items.sort_by_key(|(_, count)| *count);
        frequent_items.reverse();

        // Warm up top N items
        let warmup_count = 100.min(frequent_items.len());
        for (key, _) in frequent_items.into_iter().take(warmup_count) {
            tracing::debug!("Pre-warming frequently accessed item: {}", key);
            // In real implementation, would fetch and cache the item
        }

        Ok(())
    }
}

/// Access log entry for cache analysis
#[derive(Debug, Clone)]
pub struct AccessLogEntry {
    /// Cache key that was accessed
    pub cache_key: String,

    /// Type of cache
    pub cache_type: String,

    /// Whether it was a hit or miss
    pub was_hit: bool,

    /// Access timestamp
    pub timestamp: DateTime<Utc>,
}

use std::collections::HashMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_manager_creation() {
        let config = CacheConfig::default();
        let manager = CacheManager::new(config);

        assert_eq!(manager.total_memory_usage(), 0);
        assert!(!manager.is_memory_limit_exceeded());
    }

    #[test]
    fn test_cache_key_generation() {
        let branch_id = BranchId::main();
        let operation = Operation::Transform {
            entities: vec![EntityId::new()],
            transformation: [[1.0; 4]; 4],
        };
        let inputs = serde_json::json!({});

        let key1 = cache_key::operation_key(branch_id, &operation, &inputs);
        let key2 = cache_key::operation_key(branch_id, &operation, &inputs);

        // Same inputs should generate same key
        assert_eq!(key1, key2);

        // Different inputs should generate different keys
        let inputs2 = serde_json::json!({"param": "value"});
        let key3 = cache_key::operation_key(branch_id, &operation, &inputs2);
        assert_ne!(key1, key3);
    }
}
