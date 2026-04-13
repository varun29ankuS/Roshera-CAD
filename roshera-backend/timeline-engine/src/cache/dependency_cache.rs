//! Cache for dependency relationships

use super::CacheStats;
use crate::{ConstraintType, DependencyType, EntityId, EventId, TimelineError, TimelineResult};
use dashmap::DashMap;
use parking_lot::RwLock;
use std::sync::Arc;

/// Cached dependency information
#[derive(Debug, Clone)]
pub struct CachedDependencies {
    /// Direct dependencies (what this entity depends on)
    pub direct_dependencies: Vec<(EntityId, DependencyType)>,

    /// Reverse dependencies (what depends on this entity)
    pub reverse_dependencies: Vec<(EntityId, DependencyType)>,

    /// Transitive closure of dependencies
    pub transitive_dependencies: Vec<EntityId>,

    /// Depth in dependency graph
    pub dependency_depth: usize,
}

/// Cache for dependency relationships
pub struct DependencyCache {
    /// Entity dependency cache
    entity_deps: DashMap<EntityId, CachedDependencies>,

    /// Event to entities mapping
    event_entities: DashMap<EventId, Vec<EntityId>>,

    /// Maximum items
    max_items: usize,

    /// Cache statistics
    stats: Arc<RwLock<CacheStats>>,
}

impl DependencyCache {
    /// Create a new dependency cache
    pub fn new(max_items: usize) -> Self {
        Self {
            entity_deps: DashMap::new(),
            event_entities: DashMap::new(),
            max_items,
            stats: Arc::new(RwLock::new(CacheStats::default())),
        }
    }

    /// Get cached dependencies for an entity
    pub fn get_entity_deps(&self, entity_id: EntityId) -> Option<CachedDependencies> {
        self.update_stats(|s| {
            if self.entity_deps.contains_key(&entity_id) {
                s.hits += 1;
            } else {
                s.misses += 1;
            }
        });

        self.entity_deps.get(&entity_id).map(|entry| entry.clone())
    }

    /// Put entity dependencies in cache
    pub fn put_entity_deps(&self, entity_id: EntityId, deps: CachedDependencies) {
        // Check if we need to evict
        if self.entity_deps.len() >= self.max_items {
            self.evict_oldest();
        }

        let size = estimate_deps_size(&deps);
        let replacing = self.entity_deps.contains_key(&entity_id);

        self.entity_deps.insert(entity_id, deps);

        self.update_stats(|s| {
            if !replacing {
                s.item_count += 1;
            }
            s.size_bytes += size;
        });
    }

    /// Add dependency relationship
    pub fn add_dependency(
        &self,
        dependent: EntityId,
        dependency: EntityId,
        dep_type: DependencyType,
    ) {
        // Clone dep_type for each use
        let dep_type_clone = dep_type.clone();

        // Update direct dependencies
        self.entity_deps
            .entry(dependent)
            .and_modify(|deps| {
                if !deps
                    .direct_dependencies
                    .iter()
                    .any(|(e, _)| *e == dependency)
                {
                    deps.direct_dependencies
                        .push((dependency, dep_type.clone()));
                }
            })
            .or_insert_with(|| CachedDependencies {
                direct_dependencies: vec![(dependency, dep_type)],
                reverse_dependencies: vec![],
                transitive_dependencies: vec![],
                dependency_depth: 0,
            });

        // Update reverse dependencies
        self.entity_deps
            .entry(dependency)
            .and_modify(|deps| {
                if !deps
                    .reverse_dependencies
                    .iter()
                    .any(|(e, _)| *e == dependent)
                {
                    deps.reverse_dependencies
                        .push((dependent, dep_type_clone.clone()));
                }
            })
            .or_insert_with(|| CachedDependencies {
                direct_dependencies: vec![],
                reverse_dependencies: vec![(dependent, dep_type_clone)],
                transitive_dependencies: vec![],
                dependency_depth: 0,
            });

        // Mark transitive dependencies as needing recalculation
        self.invalidate_transitive_deps(dependent);
    }

    /// Remove dependency relationship
    pub fn remove_dependency(&self, dependent: EntityId, dependency: EntityId) {
        // Update direct dependencies
        if let Some(mut deps) = self.entity_deps.get_mut(&dependent) {
            deps.direct_dependencies.retain(|(e, _)| *e != dependency);
        }

        // Update reverse dependencies
        if let Some(mut deps) = self.entity_deps.get_mut(&dependency) {
            deps.reverse_dependencies.retain(|(e, _)| *e != dependent);
        }

        // Mark transitive dependencies as needing recalculation
        self.invalidate_transitive_deps(dependent);
    }

    /// Get all entities affected by an event
    pub fn get_event_entities(&self, event_id: EventId) -> Vec<EntityId> {
        self.event_entities
            .get(&event_id)
            .map(|entry| entry.clone())
            .unwrap_or_default()
    }

    /// Associate entities with an event
    pub fn set_event_entities(&self, event_id: EventId, entities: Vec<EntityId>) {
        self.event_entities.insert(event_id, entities);
    }

    /// Calculate transitive dependencies
    pub fn calculate_transitive_deps(&self, entity_id: EntityId) -> Vec<EntityId> {
        let mut visited = std::collections::HashSet::new();
        let mut stack = vec![entity_id];
        let mut result = Vec::new();

        while let Some(current) = stack.pop() {
            if visited.insert(current) {
                if let Some(deps) = self.entity_deps.get(&current) {
                    for (dep, _) in &deps.direct_dependencies {
                        if !visited.contains(dep) {
                            stack.push(*dep);
                            result.push(*dep);
                        }
                    }
                }
            }
        }

        result
    }

    /// Invalidate entity from cache
    pub fn invalidate_entity(&self, entity_id: EntityId) {
        if let Some((_, deps)) = self.entity_deps.remove(&entity_id) {
            let size = estimate_deps_size(&deps);

            self.update_stats(|s| {
                s.item_count = s.item_count.saturating_sub(1);
                s.size_bytes = s.size_bytes.saturating_sub(size);
            });

            // Also invalidate entities that depend on this one
            for (dependent, _) in deps.reverse_dependencies {
                self.invalidate_transitive_deps(dependent);
            }
        }
    }

    /// Clear all caches
    pub fn clear(&self) {
        self.entity_deps.clear();
        self.event_entities.clear();
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

    /// Evict oldest item (simplified - just removes first found)
    pub fn evict_oldest(&self) {
        if let Some(entry) = self.entity_deps.iter().next() {
            let entity_id = *entry.key();
            drop(entry);
            self.invalidate_entity(entity_id);
        }
    }

    /// Invalidate transitive dependencies for an entity
    fn invalidate_transitive_deps(&self, entity_id: EntityId) {
        if let Some(mut deps) = self.entity_deps.get_mut(&entity_id) {
            deps.transitive_dependencies.clear();
            deps.dependency_depth = 0;
        }

        // Also invalidate anything that depends on this
        if let Some(deps) = self.entity_deps.get(&entity_id) {
            let reverse_deps = deps.reverse_dependencies.clone();
            drop(deps);

            for (dependent, _) in reverse_deps {
                self.invalidate_transitive_deps(dependent);
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

/// Estimate size of dependency information
fn estimate_deps_size(deps: &CachedDependencies) -> usize {
    let mut size = std::mem::size_of::<CachedDependencies>();

    // Direct dependencies
    size += deps.direct_dependencies.len()
        * (std::mem::size_of::<EntityId>() + std::mem::size_of::<DependencyType>());

    // Reverse dependencies
    size += deps.reverse_dependencies.len()
        * (std::mem::size_of::<EntityId>() + std::mem::size_of::<DependencyType>());

    // Transitive dependencies
    size += deps.transitive_dependencies.len() * std::mem::size_of::<EntityId>();

    size
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dependency_cache_basic() {
        let cache = DependencyCache::new(100);

        let entity1 = EntityId::new();
        let entity2 = EntityId::new();
        let entity3 = EntityId::new();

        // Add dependencies: entity1 -> entity2 -> entity3
        cache.add_dependency(
            entity1,
            entity2,
            DependencyType::Reference {
                constraint_type: ConstraintType::Geometric,
            },
        );
        cache.add_dependency(
            entity2,
            entity3,
            DependencyType::Reference {
                constraint_type: ConstraintType::Geometric,
            },
        );

        // Check direct dependencies
        let deps1 = cache.get_entity_deps(entity1).unwrap();
        assert_eq!(deps1.direct_dependencies.len(), 1);
        assert_eq!(deps1.direct_dependencies[0].0, entity2);

        // Check reverse dependencies
        let deps2 = cache.get_entity_deps(entity2).unwrap();
        assert_eq!(deps2.reverse_dependencies.len(), 1);
        assert_eq!(deps2.reverse_dependencies[0].0, entity1);

        // Calculate transitive dependencies
        let transitive = cache.calculate_transitive_deps(entity1);
        assert_eq!(transitive.len(), 2);
        assert!(transitive.contains(&entity2));
        assert!(transitive.contains(&entity3));
    }

    #[test]
    fn test_dependency_removal() {
        let cache = DependencyCache::new(100);

        let entity1 = EntityId::new();
        let entity2 = EntityId::new();

        // Add and remove dependency
        cache.add_dependency(
            entity1,
            entity2,
            DependencyType::Reference {
                constraint_type: ConstraintType::Geometric,
            },
        );
        cache.remove_dependency(entity1, entity2);

        // Check it's removed
        let deps1 = cache.get_entity_deps(entity1).unwrap();
        assert_eq!(deps1.direct_dependencies.len(), 0);

        let deps2 = cache.get_entity_deps(entity2).unwrap();
        assert_eq!(deps2.reverse_dependencies.len(), 0);
    }

    #[test]
    fn test_event_entities() {
        let cache = DependencyCache::new(100);

        let event_id = EventId::new();
        let entities = vec![EntityId::new(), EntityId::new()];

        cache.set_event_entities(event_id, entities.clone());

        let retrieved = cache.get_event_entities(event_id);
        assert_eq!(retrieved, entities);
    }
}
