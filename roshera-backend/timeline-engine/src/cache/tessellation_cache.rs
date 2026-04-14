//! Cache for tessellated geometry (meshes)

use super::{CacheStats, CachedItem};
use crate::{BranchId, EntityId};
use chrono::{DateTime, Utc};
use dashmap::DashMap;
use lru::LruCache;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Tessellation quality levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum TessellationQuality {
    /// Low quality for preview
    Low,
    /// Medium quality for normal viewing
    Medium,
    /// High quality for close inspection
    High,
    /// Custom quality with specific parameters
    Custom {
        max_edge_length: u32,
        angle_tolerance: u32,
    },
}

/// Tessellated mesh data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TessellatedMesh {
    /// Vertex positions (x, y, z)
    pub vertices: Vec<f32>,

    /// Normal vectors (nx, ny, nz)
    pub normals: Vec<f32>,

    /// Triangle indices
    pub indices: Vec<u32>,

    /// UV coordinates (optional)
    pub uvs: Option<Vec<f32>>,

    /// Bounding box
    pub bounds: BoundingBox,

    /// Quality level used
    pub quality: TessellationQuality,

    /// Format (e.g., "threejs", "webgl", "stl")
    pub format: String,
}

/// Bounding box
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BoundingBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

/// Cache key for tessellation
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TessellationKey {
    entity_id: EntityId,
    quality: TessellationQuality,
    format: String,
}

/// Cache for tessellated geometry
pub struct TessellationCache {
    /// LRU cache for tessellations
    cache: Arc<RwLock<LruCache<TessellationKey, CachedItem<TessellatedMesh>>>>,

    /// Entity to cache keys mapping for invalidation
    entity_keys: DashMap<EntityId, Vec<TessellationKey>>,

    /// Branch to entities mapping
    branch_entities: DashMap<BranchId, Vec<EntityId>>,

    /// Maximum items
    max_items: usize,

    /// TTL for cached items
    ttl_seconds: u64,

    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
}

impl TessellationCache {
    /// Create a new tessellation cache
    pub fn new(max_items: usize, ttl_seconds: u64) -> Self {
        Self {
            cache: Arc::new(RwLock::new(LruCache::new(
                std::num::NonZeroUsize::new(max_items).unwrap(),
            ))),
            entity_keys: DashMap::new(),
            branch_entities: DashMap::new(),
            max_items,
            ttl_seconds,
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Get cached tessellation
    pub fn get(
        &self,
        entity_id: EntityId,
        quality: TessellationQuality,
        format: &str,
    ) -> Option<TessellatedMesh> {
        let key = TessellationKey {
            entity_id,
            quality,
            format: format.to_string(),
        };

        let mut cache = self.cache.write();

        if let Some(item) = cache.get_mut(&key) {
            // Check if expired
            let age = Utc::now() - item.cached_at;
            if age.num_seconds() as u64 > self.ttl_seconds {
                cache.pop(&key);
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

    /// Put tessellation in cache
    pub fn put(&self, entity_id: EntityId, branch_id: BranchId, mesh: TessellatedMesh) {
        let key = TessellationKey {
            entity_id,
            quality: mesh.quality,
            format: mesh.format.clone(),
        };

        let size_bytes = estimate_mesh_size(&mesh);

        let item = CachedItem {
            value: mesh,
            cached_at: Utc::now(),
            size_bytes,
            access_count: 0,
            last_accessed: Utc::now(),
        };

        // Update mappings
        self.entity_keys
            .entry(entity_id)
            .or_insert_with(Vec::new)
            .push(key.clone());

        self.branch_entities
            .entry(branch_id)
            .or_insert_with(Vec::new)
            .push(entity_id);

        // Insert into cache
        let mut cache = self.cache.write();

        let replacing = cache.contains(&key);

        if cache.push(key, item).is_some() {
            self.update_stats(|s| {
                s.evictions += 1;
                // Size tracking is handled separately
            });
        }

        self.update_stats(|s| {
            if !replacing {
                s.item_count += 1;
            }
            s.size_bytes += size_bytes;
        });
    }

    /// Invalidate all tessellations for an entity
    pub fn invalidate_entity(&self, entity_id: EntityId) {
        if let Some((_, keys)) = self.entity_keys.remove(&entity_id) {
            let mut cache = self.cache.write();

            for key in keys {
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
        if let Some((_, entities)) = self.branch_entities.remove(&branch_id) {
            for entity_id in entities {
                self.invalidate_entity(entity_id);
            }
        }
    }

    /// Clear all caches
    pub fn clear(&self) {
        self.cache.write().clear();
        self.entity_keys.clear();
        self.branch_entities.clear();
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

    /// Evict oldest item
    pub fn evict_oldest(&self) {
        let mut cache = self.cache.write();

        // Find oldest item
        let mut oldest: Option<(TessellationKey, DateTime<Utc>)> = None;

        for (key, item) in cache.iter() {
            if oldest.is_none() || item.last_accessed < oldest.as_ref().unwrap().1 {
                oldest = Some((key.clone(), item.last_accessed));
            }
        }

        // Evict it
        if let Some((key, _)) = oldest {
            if let Some(item) = cache.pop(&key) {
                // Update entity mapping
                if let Some(mut entry) = self.entity_keys.get_mut(&key.entity_id) {
                    entry.retain(|k| k != &key);
                }

                self.update_stats(|s| {
                    s.evictions += 1;
                    s.item_count = s.item_count.saturating_sub(1);
                    s.size_bytes = s.size_bytes.saturating_sub(item.size_bytes);
                });
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

/// Estimate size of a tessellated mesh
fn estimate_mesh_size(mesh: &TessellatedMesh) -> usize {
    let mut size = std::mem::size_of::<TessellatedMesh>();

    // Vertices: 3 floats per vertex
    size += mesh.vertices.len() * std::mem::size_of::<f32>();

    // Normals: 3 floats per normal
    size += mesh.normals.len() * std::mem::size_of::<f32>();

    // Indices: 1 u32 per index
    size += mesh.indices.len() * std::mem::size_of::<u32>();

    // UVs: 2 floats per UV (if present)
    if let Some(ref uvs) = mesh.uvs {
        size += uvs.len() * std::mem::size_of::<f32>();
    }

    // Format string
    size += mesh.format.len();

    size
}

impl TessellationQuality {
    /// Get parameters for tessellation
    pub fn parameters(&self) -> (f64, f64) {
        match self {
            TessellationQuality::Low => (10.0, 0.5),
            TessellationQuality::Medium => (5.0, 0.1),
            TessellationQuality::High => (1.0, 0.05),
            TessellationQuality::Custom {
                max_edge_length,
                angle_tolerance,
            } => (*max_edge_length as f64, *angle_tolerance as f64 / 1000.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tessellation_cache_basic() {
        let cache = TessellationCache::new(100, 3600);
        let entity_id = EntityId::new();
        let branch_id = BranchId::main();

        let mesh = TessellatedMesh {
            vertices: vec![0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0],
            normals: vec![0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
            indices: vec![0, 1, 2],
            uvs: None,
            bounds: BoundingBox {
                min: [0.0, 0.0, 0.0],
                max: [1.0, 1.0, 0.0],
            },
            quality: TessellationQuality::Medium,
            format: "threejs".to_string(),
        };

        // Test miss
        assert!(cache
            .get(entity_id, TessellationQuality::Medium, "threejs")
            .is_none());

        // Test put and hit
        cache.put(entity_id, branch_id, mesh.clone());

        let cached = cache.get(entity_id, TessellationQuality::Medium, "threejs");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().vertices, mesh.vertices);

        // Check stats
        let stats = cache.get_stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.item_count, 1);
    }

    #[test]
    fn test_quality_parameters() {
        let (edge, angle) = TessellationQuality::Low.parameters();
        assert_eq!(edge, 10.0);
        assert_eq!(angle, 0.5);

        let custom = TessellationQuality::Custom {
            max_edge_length: 25,
            angle_tolerance: 100,
        };
        let (edge, angle) = custom.parameters();
        assert_eq!(edge, 25.0);
        assert_eq!(angle, 0.1);
    }
}
