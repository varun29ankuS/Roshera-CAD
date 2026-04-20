//! High-performance caching layer for session management
//!
//! Provides multi-layer caching with LRU eviction, TTL support,
//! and automatic cache warming for frequently accessed data.

use crate::permissions::UserPermissions;
use dashmap::DashMap;
use lru::LruCache;
use serde::{Deserialize, Serialize};
use shared_types::{
    CADObject, CommandResult, GeometryId, OrientationCubeState, SessionError, SessionState,
    SketchState,
};
use std::hash::Hash;
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Cache entry with TTL and metadata
#[derive(Debug, Clone)]
pub struct CacheEntry<T> {
    /// Cached value
    pub value: T,
    /// When entry was created
    pub created_at: Instant,
    /// When entry was last accessed
    pub last_accessed: Instant,
    /// Time to live
    pub ttl: Option<Duration>,
    /// Access count
    pub access_count: u64,
    /// Size in bytes (estimated)
    pub size_bytes: usize,
}

impl<T> CacheEntry<T> {
    /// Create new cache entry
    pub fn new(value: T, ttl: Option<Duration>, size_bytes: usize) -> Self {
        let now = Instant::now();
        Self {
            value,
            created_at: now,
            last_accessed: now,
            ttl,
            access_count: 0,
            size_bytes,
        }
    }

    /// Check if entry is expired
    pub fn is_expired(&self) -> bool {
        if let Some(ttl) = self.ttl {
            self.created_at.elapsed() > ttl
        } else {
            false
        }
    }

    /// Update last accessed time
    pub fn touch(&mut self) {
        self.last_accessed = Instant::now();
        self.access_count += 1;
    }
}

/// Cache statistics
#[derive(Debug, Clone, Default)]
pub struct CacheStats {
    /// Total hits
    pub hits: u64,
    /// Total misses
    pub misses: u64,
    /// Total evictions
    pub evictions: u64,
    /// Current size in bytes
    pub size_bytes: usize,
    /// Current entry count
    pub entry_count: usize,
}

impl CacheStats {
    /// Calculate hit ratio
    pub fn hit_ratio(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

/// LRU cache with TTL support
pub struct TtlLruCache<K: Eq + Hash, V> {
    /// Internal LRU cache
    cache: RwLock<LruCache<K, CacheEntry<V>>>,
    /// Cache statistics
    stats: RwLock<CacheStats>,
    /// Maximum size in bytes
    max_size_bytes: usize,
    /// Default TTL
    default_ttl: Option<Duration>,
}

impl<K: Eq + Hash + Clone, V: Clone> TtlLruCache<K, V> {
    /// Create new cache
    pub fn new(capacity: usize, max_size_bytes: usize, default_ttl: Option<Duration>) -> Self {
        Self {
            cache: RwLock::new(LruCache::new(NonZeroUsize::new(capacity).unwrap())),
            stats: RwLock::new(CacheStats::default()),
            max_size_bytes,
            default_ttl,
        }
    }

    /// Get value from cache
    pub async fn get(&self, key: &K) -> Option<V> {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;

        if let Some(entry) = cache.get_mut(key) {
            if entry.is_expired() {
                cache.pop(key);
                stats.misses += 1;
                None
            } else {
                entry.touch();
                stats.hits += 1;
                Some(entry.value.clone())
            }
        } else {
            stats.misses += 1;
            None
        }
    }

    /// Put value in cache
    pub async fn put(&self, key: K, value: V, size_bytes: usize) {
        self.put_with_ttl(key, value, size_bytes, self.default_ttl)
            .await;
    }

    /// Put value with custom TTL
    pub async fn put_with_ttl(&self, key: K, value: V, size_bytes: usize, ttl: Option<Duration>) {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;

        // Check size constraints
        if stats.size_bytes + size_bytes > self.max_size_bytes {
            // Evict entries until we have space
            while stats.size_bytes + size_bytes > self.max_size_bytes && cache.len() > 0 {
                if let Some((_, evicted)) = cache.pop_lru() {
                    stats.size_bytes = stats.size_bytes.saturating_sub(evicted.size_bytes);
                    stats.evictions += 1;
                }
            }
        }

        let entry = CacheEntry::new(value, ttl, size_bytes);
        if let Some(old_entry) = cache.put(key, entry) {
            stats.size_bytes = stats.size_bytes.saturating_sub(old_entry.size_bytes);
        }
        stats.size_bytes += size_bytes;
        stats.entry_count = cache.len();
    }

    /// Remove value from cache
    pub async fn remove(&self, key: &K) -> Option<V> {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;

        if let Some(entry) = cache.pop(key) {
            stats.size_bytes = stats.size_bytes.saturating_sub(entry.size_bytes);
            stats.entry_count = cache.len();
            Some(entry.value)
        } else {
            None
        }
    }

    /// Clear cache
    pub async fn clear(&self) {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;

        cache.clear();
        stats.size_bytes = 0;
        stats.entry_count = 0;
    }

    /// Get cache statistics
    pub async fn stats(&self) -> CacheStats {
        self.stats.read().await.clone()
    }

    /// Clean expired entries
    pub async fn clean_expired(&self) {
        let mut cache = self.cache.write().await;
        let mut stats = self.stats.write().await;
        let mut to_remove = Vec::new();

        // Collect expired keys
        for (key, entry) in cache.iter() {
            if entry.is_expired() {
                to_remove.push(key.clone());
            }
        }

        // Remove expired entries
        for key in to_remove {
            if let Some(entry) = cache.pop(&key) {
                stats.size_bytes = stats.size_bytes.saturating_sub(entry.size_bytes);
                stats.evictions += 1;
            }
        }

        stats.entry_count = cache.len();
    }
}

/// Multi-layer cache manager
pub struct CacheManager {
    /// Session cache
    sessions: Arc<TtlLruCache<String, SessionState>>,
    /// Object cache (session_id:object_id -> object)
    objects: Arc<TtlLruCache<String, CADObject>>,
    /// Permission cache (session_id:user_id -> permissions)
    permissions: Arc<TtlLruCache<String, UserPermissions>>,
    /// Command result cache
    command_results: Arc<TtlLruCache<String, CommandResult>>,
    /// Computed geometry cache (for expensive operations)
    computed_geometry: Arc<DashMap<String, CacheEntry<serde_json::Value>>>,
    /// Cache configuration
    config: CacheConfig,
}

/// Cache configuration
#[derive(Debug, Clone)]
pub struct CacheConfig {
    /// Session cache capacity
    pub session_capacity: usize,
    /// Object cache capacity
    pub object_capacity: usize,
    /// Permission cache capacity
    pub permission_capacity: usize,
    /// Command result cache capacity
    pub command_capacity: usize,
    /// Maximum cache size in MB
    pub max_size_mb: usize,
    /// Default TTL for sessions
    pub session_ttl: Duration,
    /// Default TTL for objects
    pub object_ttl: Duration,
    /// Default TTL for permissions
    pub permission_ttl: Duration,
    /// Default TTL for commands
    pub command_ttl: Duration,
    /// Enable cache warming
    pub enable_warming: bool,
    /// Cleanup interval
    pub cleanup_interval: Duration,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            session_capacity: 1000,
            object_capacity: 10000,
            permission_capacity: 5000,
            command_capacity: 10000,
            max_size_mb: 512,
            session_ttl: Duration::from_secs(3600),   // 1 hour
            object_ttl: Duration::from_secs(1800),    // 30 minutes
            permission_ttl: Duration::from_secs(300), // 5 minutes
            command_ttl: Duration::from_secs(60),     // 1 minute
            enable_warming: true,
            cleanup_interval: Duration::from_secs(300), // 5 minutes
        }
    }
}

impl CacheManager {
    /// Create new cache manager
    pub fn new(config: CacheConfig) -> Self {
        let max_bytes = config.max_size_mb * 1024 * 1024;

        Self {
            sessions: Arc::new(TtlLruCache::new(
                config.session_capacity,
                max_bytes / 4,
                Some(config.session_ttl),
            )),
            objects: Arc::new(TtlLruCache::new(
                config.object_capacity,
                max_bytes / 2,
                Some(config.object_ttl),
            )),
            permissions: Arc::new(TtlLruCache::new(
                config.permission_capacity,
                max_bytes / 8,
                Some(config.permission_ttl),
            )),
            command_results: Arc::new(TtlLruCache::new(
                config.command_capacity,
                max_bytes / 8,
                Some(config.command_ttl),
            )),
            computed_geometry: Arc::new(DashMap::new()),
            config,
        }
    }

    /// Start cache maintenance task
    pub fn start_maintenance(&self) {
        let sessions = self.sessions.clone();
        let objects = self.objects.clone();
        let permissions = self.permissions.clone();
        let command_results = self.command_results.clone();
        let computed_geometry = self.computed_geometry.clone();
        let interval = self.config.cleanup_interval;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(interval);

            loop {
                interval.tick().await;

                // Clean expired entries
                sessions.clean_expired().await;
                objects.clean_expired().await;
                permissions.clean_expired().await;
                command_results.clean_expired().await;

                // Clean computed geometry
                let now = Instant::now();
                computed_geometry.retain(|_, entry| !entry.is_expired());

                debug!("Cache cleanup completed");
            }
        });
    }

    // Session cache operations

    /// Get session from cache
    pub async fn get_session(&self, session_id: &str) -> Option<SessionState> {
        self.sessions.get(&session_id.to_string()).await
    }

    /// Cache session
    pub async fn cache_session(&self, session: &SessionState) {
        let size = estimate_size(session);
        self.sessions
            .put(session.id.to_string(), session.clone(), size)
            .await;
    }

    /// Invalidate session
    pub async fn invalidate_session(&self, session_id: &str) {
        self.sessions.remove(&session_id.to_string()).await;

        // Also invalidate related caches
        let prefix = format!("{}:", session_id);

        // This is simplified - in production you'd track keys more efficiently
        self.objects.clear().await;
        self.permissions.clear().await;
    }

    // Object cache operations

    /// Get object from cache
    pub async fn get_object(&self, session_id: &str, object_id: &GeometryId) -> Option<CADObject> {
        let key = format!("{}:{:?}", session_id, object_id);
        self.objects.get(&key).await
    }

    /// Cache object
    pub async fn cache_object(&self, session_id: &str, object: &CADObject) {
        let key = format!("{}:{}", session_id, object.id);
        let size = estimate_size(object);
        self.objects.put(key, object.clone(), size).await;
    }

    /// Invalidate object
    pub async fn invalidate_object(&self, session_id: &str, object_id: &GeometryId) {
        let key = format!("{}:{:?}", session_id, object_id);
        self.objects.remove(&key).await;
    }

    // Permission cache operations

    /// Get permissions from cache
    pub async fn get_permissions(
        &self,
        session_id: &str,
        user_id: &str,
    ) -> Option<UserPermissions> {
        let key = format!("{}:{}", session_id, user_id);
        self.permissions.get(&key).await
    }

    /// Cache permissions
    pub async fn cache_permissions(&self, session_id: &str, permissions: &UserPermissions) {
        let key = format!("{}:{}", session_id, permissions.user_id);
        let size = estimate_size(permissions);
        self.permissions.put(key, permissions.clone(), size).await;
    }

    /// Invalidate permissions
    pub async fn invalidate_permissions(&self, session_id: &str, user_id: &str) {
        let key = format!("{}:{}", session_id, user_id);
        self.permissions.remove(&key).await;
    }

    // Command result cache

    /// Get command result from cache
    pub async fn get_command_result(&self, command_hash: &str) -> Option<CommandResult> {
        self.command_results.get(&command_hash.to_string()).await
    }

    /// Cache command result
    pub async fn cache_command_result(&self, command_hash: &str, result: &CommandResult) {
        let size = estimate_size(result);
        self.command_results
            .put(command_hash.to_string(), result.clone(), size)
            .await;
    }

    // Computed geometry cache

    /// Get computed geometry from cache
    pub fn get_computed_geometry(&self, key: &str) -> Option<serde_json::Value> {
        self.computed_geometry.get(key).and_then(|entry| {
            if entry.is_expired() {
                None
            } else {
                Some(entry.value.clone())
            }
        })
    }

    /// Cache computed geometry
    pub fn cache_computed_geometry(&self, key: String, value: serde_json::Value, ttl: Duration) {
        let size = estimate_size(&value);
        let entry = CacheEntry::new(value, Some(ttl), size);
        self.computed_geometry.insert(key, entry);
    }

    // Cache statistics

    /// Get cache statistics
    pub async fn get_stats(&self) -> CacheManagerStats {
        CacheManagerStats {
            sessions: self.sessions.stats().await,
            objects: self.objects.stats().await,
            permissions: self.permissions.stats().await,
            command_results: self.command_results.stats().await,
            computed_geometry_count: self.computed_geometry.len(),
        }
    }

    /// Warm cache with frequently accessed data
    pub async fn warm_cache<F, Fut>(&self, loader: F) -> Result<(), SessionError>
    where
        F: Fn() -> Fut,
        Fut: std::future::Future<Output = Result<WarmupData, SessionError>>,
    {
        if !self.config.enable_warming {
            return Ok(());
        }

        info!("Starting cache warmup");
        let warmup_data = loader().await?;

        // Warm session cache
        for session in warmup_data.sessions {
            self.cache_session(&session).await;
        }

        // Warm object cache
        for (session_id, objects) in warmup_data.objects {
            for object in objects {
                self.cache_object(&session_id, &object).await;
            }
        }

        // Warm permission cache
        for (session_id, permissions) in warmup_data.permissions {
            for perm in permissions {
                self.cache_permissions(&session_id, &perm).await;
            }
        }

        info!("Cache warmup completed");
        Ok(())
    }

    /// Clear all caches
    pub async fn clear_all(&self) {
        self.sessions.clear().await;
        self.objects.clear().await;
        self.permissions.clear().await;
        self.command_results.clear().await;
        self.computed_geometry.clear();

        info!("All caches cleared");
    }
}

/// Data for cache warming
#[derive(Debug)]
pub struct WarmupData {
    /// Sessions to warm
    pub sessions: Vec<SessionState>,
    /// Objects to warm (grouped by session)
    pub objects: Vec<(String, Vec<CADObject>)>,
    /// Permissions to warm (grouped by session)
    pub permissions: Vec<(String, Vec<UserPermissions>)>,
}

/// Cache manager statistics
#[derive(Debug, Clone)]
pub struct CacheManagerStats {
    /// Session cache stats
    pub sessions: CacheStats,
    /// Object cache stats
    pub objects: CacheStats,
    /// Permission cache stats
    pub permissions: CacheStats,
    /// Command result cache stats
    pub command_results: CacheStats,
    /// Computed geometry count
    pub computed_geometry_count: usize,
}

impl CacheManagerStats {
    /// Get total memory usage
    pub fn total_memory_bytes(&self) -> usize {
        self.sessions.size_bytes
            + self.objects.size_bytes
            + self.permissions.size_bytes
            + self.command_results.size_bytes
    }

    /// Get overall hit ratio
    pub fn overall_hit_ratio(&self) -> f64 {
        let total_hits = self.sessions.hits
            + self.objects.hits
            + self.permissions.hits
            + self.command_results.hits;

        let total_misses = self.sessions.misses
            + self.objects.misses
            + self.permissions.misses
            + self.command_results.misses;

        let total = total_hits + total_misses;
        if total == 0 {
            0.0
        } else {
            total_hits as f64 / total as f64
        }
    }
}

/// Estimate size of a value in bytes
fn estimate_size<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value).map(|v| v.len()).unwrap_or(1024) // Default estimate
}

/// Create cache key for geometry operations
pub fn geometry_cache_key(operation: &str, params: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(operation.as_bytes());
    hasher.update(params.to_string().as_bytes());
    format!("geom:{:x}", hasher.finalize())
}

/// Decorator for cacheable operations
pub async fn with_cache<T, F, Fut>(
    cache: &CacheManager,
    key: &str,
    ttl: Duration,
    compute: F,
) -> Result<T, SessionError>
where
    T: Serialize + for<'de> Deserialize<'de> + Clone,
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<T, SessionError>>,
{
    // Check cache first
    if let Some(cached) = cache.get_computed_geometry(key) {
        if let Ok(value) = serde_json::from_value(cached) {
            debug!("Cache hit for key: {}", key);
            return Ok(value);
        }
    }

    // Compute value
    debug!("Cache miss for key: {}", key);
    let value = compute().await?;

    // Cache result
    if let Ok(json_value) = serde_json::to_value(&value) {
        cache.cache_computed_geometry(key.to_string(), json_value, ttl);
    }

    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_lru_cache() {
        let cache: TtlLruCache<String, String> =
            TtlLruCache::new(2, 1024, Some(Duration::from_secs(60)));

        // Test basic operations
        cache
            .put("key1".to_string(), "value1".to_string(), 10)
            .await;
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        // Test LRU eviction
        cache
            .put("key2".to_string(), "value2".to_string(), 10)
            .await;
        cache
            .put("key3".to_string(), "value3".to_string(), 10)
            .await;

        // key1 should be evicted
        assert_eq!(cache.get(&"key1".to_string()).await, None);
        assert_eq!(
            cache.get(&"key2".to_string()).await,
            Some("value2".to_string())
        );
        assert_eq!(
            cache.get(&"key3".to_string()).await,
            Some("value3".to_string())
        );

        // Test stats
        let stats = cache.stats().await;
        assert_eq!(stats.hits, 2);
        assert_eq!(stats.misses, 1);
    }

    #[tokio::test]
    async fn test_ttl_expiration() {
        let cache: TtlLruCache<String, String> = TtlLruCache::new(10, 1024, None);

        // Put with short TTL
        cache
            .put_with_ttl(
                "key1".to_string(),
                "value1".to_string(),
                10,
                Some(Duration::from_millis(100)),
            )
            .await;

        // Should exist immediately
        assert_eq!(
            cache.get(&"key1".to_string()).await,
            Some("value1".to_string())
        );

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Should be expired
        assert_eq!(cache.get(&"key1".to_string()).await, None);
    }

    #[tokio::test]
    async fn test_cache_manager() {
        let config = CacheConfig::default();
        let manager = CacheManager::new(config);

        // Test session caching
        let session = SessionState {
            id: uuid::Uuid::new_v4(),
            name: "Test Session".to_string(),
            owner_id: "test-owner".to_string(),
            objects: std::collections::HashMap::new(),
            history: std::collections::VecDeque::new(),
            history_index: 0,
            created_at: 1000,
            modified_at: 2000,
            active_users: vec![],
            settings: Default::default(),
            metadata: std::collections::HashMap::new(),
            sketch_planes: std::collections::HashMap::new(),
            active_sketch_plane: None,
            orientation_cube: OrientationCubeState::default(),
            sketch_state: SketchState::default(),
        };

        manager.cache_session(&session).await;

        let cached = manager.get_session(&session.id.to_string()).await;
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().name, "Test Session");

        // Test stats
        let stats = manager.get_stats().await;
        assert_eq!(stats.sessions.hits, 0);
        assert_eq!(stats.sessions.misses, 0);
        assert_eq!(stats.sessions.entry_count, 1);
    }
}
