//! Tessellation caching.
//!
//! LRU eviction with content-based hashing.

use super::{TessellationParams, ThreeJsMesh};
use crate::primitives::{face::FaceId, shell::ShellId, solid::SolidId, surface::Surface};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, RwLock};

/// Cache key for tessellation results
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CacheKey {
    entity_type: EntityType,
    entity_id: u64,
    params_hash: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum EntityType {
    Face,
    Shell,
    Solid,
    Surface,
}

/// Tessellation cache with LRU eviction
pub struct TessellationCache {
    cache: Arc<RwLock<LruCache<CacheKey, Arc<ThreeJsMesh>>>>,
    max_size: usize,
    hits: Arc<RwLock<u64>>,
    misses: Arc<RwLock<u64>>,
}

impl TessellationCache {
    /// Create a new cache with specified maximum size
    pub fn new(max_size: usize) -> Self {
        Self {
            cache: Arc::new(RwLock::new(LruCache::new(max_size))),
            max_size,
            hits: Arc::new(RwLock::new(0)),
            misses: Arc::new(RwLock::new(0)),
        }
    }

    /// Get a cached mesh if available
    pub fn get(&self, key: &CacheKey) -> Option<Arc<ThreeJsMesh>> {
        let mut cache = self.cache.write().expect("tessellation cache RwLock poisoned");
        if let Some(mesh) = cache.get(key) {
            *self.hits.write().expect("tessellation hits RwLock poisoned") += 1;
            Some(Arc::clone(mesh))
        } else {
            *self.misses.write().expect("tessellation misses RwLock poisoned") += 1;
            None
        }
    }

    /// Insert a mesh into the cache
    pub fn insert(&self, key: CacheKey, mesh: ThreeJsMesh) {
        let mut cache = self.cache.write().expect("tessellation cache RwLock poisoned");
        cache.put(key, Arc::new(mesh));
    }

    /// Clear the entire cache
    pub fn clear(&self) {
        let mut cache = self.cache.write().expect("tessellation cache RwLock poisoned");
        cache.clear();
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        let hits = *self.hits.read().expect("tessellation hits RwLock poisoned");
        let misses = *self.misses.read().expect("tessellation misses RwLock poisoned");
        let size = self.cache.read().expect("tessellation cache RwLock poisoned").len();

        CacheStats {
            hits,
            misses,
            hit_rate: if hits + misses > 0 {
                hits as f64 / (hits + misses) as f64
            } else {
                0.0
            },
            size,
            max_size: self.max_size,
        }
    }

    /// Create a cache key for a face
    pub fn face_key(face_id: FaceId, params: &TessellationParams) -> CacheKey {
        CacheKey {
            entity_type: EntityType::Face,
            entity_id: face_id as u64,
            params_hash: hash_params(params),
        }
    }

    /// Create a cache key for a shell
    pub fn shell_key(shell_id: ShellId, params: &TessellationParams) -> CacheKey {
        CacheKey {
            entity_type: EntityType::Shell,
            entity_id: shell_id as u64,
            params_hash: hash_params(params),
        }
    }

    /// Create a cache key for a solid
    pub fn solid_key(solid_id: SolidId, params: &TessellationParams) -> CacheKey {
        CacheKey {
            entity_type: EntityType::Solid,
            entity_id: solid_id as u64,
            params_hash: hash_params(params),
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub hits: u64,
    pub misses: u64,
    pub hit_rate: f64,
    pub size: usize,
    pub max_size: usize,
}

/// Simple LRU cache implementation
struct LruCache<K: Eq + Hash, V> {
    map: HashMap<K, (V, usize)>,
    order: Vec<K>,
    capacity: usize,
    counter: usize,
}

impl<K: Eq + Hash + Clone, V> LruCache<K, V> {
    fn new(capacity: usize) -> Self {
        Self {
            map: HashMap::with_capacity(capacity),
            order: Vec::with_capacity(capacity),
            capacity,
            counter: 0,
        }
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        if self.map.contains_key(key) {
            self.counter += 1;
            let counter = self.counter;
            self.map
                .get_mut(key)
                .expect("LruCache: contains_key verified above that key is present")
                .1 = counter;
            self.map.get(key).map(|(value, _)| value)
        } else {
            None
        }
    }

    fn put(&mut self, key: K, value: V) {
        if self.map.len() >= self.capacity && !self.map.contains_key(&key) {
            // Evict least recently used
            if let Some(lru_key) = self.find_lru() {
                self.map.remove(&lru_key);
            }
        }

        self.counter += 1;
        self.map.insert(key, (value, self.counter));
    }

    fn find_lru(&self) -> Option<K> {
        self.map
            .iter()
            .min_by_key(|(_, (_, counter))| counter)
            .map(|(key, _)| key.clone())
    }

    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
        self.counter = 0;
    }

    fn len(&self) -> usize {
        self.map.len()
    }
}

/// Hash tessellation parameters for cache key
fn hash_params(params: &TessellationParams) -> u64 {
    let mut hasher = DefaultHasher::new();

    // Hash all relevant parameters
    ((params.max_edge_length * 1000.0) as u32).hash(&mut hasher);
    ((params.max_angle_deviation * 1000.0) as u32).hash(&mut hasher);
    ((params.chord_tolerance * 10000.0) as u32).hash(&mut hasher);
    params.min_segments.hash(&mut hasher);
    params.max_segments.hash(&mut hasher);

    hasher.finish()
}

/// Level of Detail (LOD) cache for multi-resolution meshes
pub struct LodCache {
    levels: Arc<RwLock<HashMap<(SolidId, LodLevel), Arc<ThreeJsMesh>>>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LodLevel {
    Ultra = 0, // Highest quality
    High = 1,
    Medium = 2,
    Low = 3,
    Preview = 4, // Lowest quality
}

impl LodCache {
    pub fn new() -> Self {
        Self {
            levels: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get mesh at specific LOD level
    pub fn get(&self, solid_id: SolidId, level: LodLevel) -> Option<Arc<ThreeJsMesh>> {
        let cache = self.levels.read().expect("LOD cache RwLock poisoned");
        cache.get(&(solid_id, level)).cloned()
    }

    /// Store mesh at specific LOD level
    pub fn insert(&self, solid_id: SolidId, level: LodLevel, mesh: ThreeJsMesh) {
        let mut cache = self.levels.write().expect("LOD cache RwLock poisoned");
        cache.insert((solid_id, level), Arc::new(mesh));
    }

    /// Get the best available LOD that meets quality requirements
    pub fn get_best_available(
        &self,
        solid_id: SolidId,
        min_level: LodLevel,
    ) -> Option<(LodLevel, Arc<ThreeJsMesh>)> {
        let cache = self.levels.read().expect("LOD cache RwLock poisoned");

        // Try from requested level up to highest quality
        for level in [min_level, LodLevel::Medium, LodLevel::High, LodLevel::Ultra] {
            if let Some(mesh) = cache.get(&(solid_id, level)) {
                return Some((level, Arc::clone(mesh)));
            }
        }

        // Fall back to lower quality if necessary
        for level in [LodLevel::Low, LodLevel::Preview] {
            if let Some(mesh) = cache.get(&(solid_id, level)) {
                return Some((level, Arc::clone(mesh)));
            }
        }

        None
    }
}

/// Incremental tessellation cache for progressive refinement
pub struct IncrementalCache {
    partial_results: Arc<RwLock<HashMap<CacheKey, PartialTessellation>>>,
}

struct PartialTessellation {
    completed_faces: Vec<FaceId>,
    mesh: ThreeJsMesh,
    progress: f32,
}

impl IncrementalCache {
    pub fn new() -> Self {
        Self {
            partial_results: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get partial tessellation if available
    pub fn get_partial(&self, key: &CacheKey) -> Option<(ThreeJsMesh, f32)> {
        let cache = self.partial_results.read().expect("incremental tessellation cache RwLock poisoned");
        cache
            .get(key)
            .map(|partial| (partial.mesh.clone(), partial.progress))
    }

    /// Update partial tessellation
    pub fn update_partial(
        &self,
        key: CacheKey,
        face_id: FaceId,
        face_mesh: &ThreeJsMesh,
        total_faces: usize,
    ) {
        let mut cache = self.partial_results.write().expect("incremental tessellation cache RwLock poisoned");
        let partial = cache.entry(key).or_insert_with(|| PartialTessellation {
            completed_faces: Vec::new(),
            mesh: ThreeJsMesh::new(),
            progress: 0.0,
        });

        partial.completed_faces.push(face_id);
        partial.mesh.merge(face_mesh);
        partial.progress = partial.completed_faces.len() as f32 / total_faces as f32;
    }

    /// Mark tessellation as complete and remove from partial cache
    pub fn complete(&self, key: &CacheKey) {
        let mut cache = self.partial_results.write().expect("incremental tessellation cache RwLock poisoned");
        cache.remove(key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lru_cache() {
        let mut cache = LruCache::new(2);
        cache.put("a", 1);
        cache.put("b", 2);

        assert_eq!(cache.get(&"a"), Some(&1));

        cache.put("c", 3); // Should evict "b"
        assert_eq!(cache.get(&"b"), None);
        assert_eq!(cache.get(&"a"), Some(&1));
        assert_eq!(cache.get(&"c"), Some(&3));
    }

    #[test]
    fn test_cache_stats() {
        let cache = TessellationCache::new(100);
        let key = CacheKey {
            entity_type: EntityType::Face,
            entity_id: 1,
            params_hash: 12345,
        };

        // Miss
        assert!(cache.get(&key).is_none());

        // Insert
        cache.insert(key.clone(), ThreeJsMesh::new());

        // Hit
        assert!(cache.get(&key).is_some());

        let stats = cache.stats();
        assert_eq!(stats.hits, 1);
        assert_eq!(stats.misses, 1);
        assert_eq!(stats.hit_rate, 0.5);
    }
}
