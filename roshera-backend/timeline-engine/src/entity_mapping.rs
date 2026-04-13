//! Entity ID to Geometry ID mapping system
//!
//! This module provides bidirectional mapping between Timeline EntityIds
//! and Geometry Engine IDs, ensuring proper tracking of geometry throughout
//! the timeline system.

use crate::{EntityId, TimelineError, TimelineResult};
use dashmap::DashMap;
use geometry_engine::primitives::{
    edge::EdgeId, face::FaceId, solid::SolidId, topology_builder::GeometryId as GeometryEngineId,
    vertex::VertexId,
};
use std::sync::Arc;
use uuid::Uuid;

/// Mapping system for entity IDs
pub struct EntityMapping {
    /// Map from Timeline EntityId to Geometry Engine ID
    entity_to_geometry: Arc<DashMap<EntityId, GeometryEngineId>>,

    /// Reverse map from Geometry Engine ID to Timeline EntityId
    geometry_to_entity: Arc<DashMap<String, EntityId>>,

    /// Map from EntityId to the owning solid (for edges, faces, etc.)
    entity_parent_solid: Arc<DashMap<EntityId, SolidId>>,
}

impl EntityMapping {
    /// Create a new entity mapping system
    pub fn new() -> Self {
        Self {
            entity_to_geometry: Arc::new(DashMap::new()),
            geometry_to_entity: Arc::new(DashMap::new()),
            entity_parent_solid: Arc::new(DashMap::new()),
        }
    }

    /// Register a solid mapping
    pub fn register_solid(&self, entity_id: EntityId, solid_id: SolidId) {
        let geom_id = GeometryEngineId::Solid(solid_id);
        self.entity_to_geometry.insert(entity_id, geom_id);
        self.geometry_to_entity
            .insert(format!("solid_{}", solid_id), entity_id);
    }

    /// Register a face mapping
    pub fn register_face(&self, entity_id: EntityId, face_id: FaceId, parent_solid: SolidId) {
        let geom_id = GeometryEngineId::Face(face_id);
        self.entity_to_geometry.insert(entity_id, geom_id);
        self.geometry_to_entity
            .insert(format!("face_{}", face_id), entity_id);
        self.entity_parent_solid.insert(entity_id, parent_solid);
    }

    /// Register an edge mapping
    pub fn register_edge(&self, entity_id: EntityId, edge_id: EdgeId, parent_solid: SolidId) {
        let geom_id = GeometryEngineId::Edge(edge_id);
        self.entity_to_geometry.insert(entity_id, geom_id);
        self.geometry_to_entity
            .insert(format!("edge_{}", edge_id), entity_id);
        self.entity_parent_solid.insert(entity_id, parent_solid);
    }

    /// Register a vertex mapping
    pub fn register_vertex(&self, entity_id: EntityId, vertex_id: VertexId, parent_solid: SolidId) {
        let geom_id = GeometryEngineId::Vertex(vertex_id);
        self.entity_to_geometry.insert(entity_id, geom_id);
        self.geometry_to_entity
            .insert(format!("vertex_{}", vertex_id), entity_id);
        self.entity_parent_solid.insert(entity_id, parent_solid);
    }

    /// Get geometry ID from entity ID
    pub fn get_geometry_id(&self, entity_id: EntityId) -> Option<GeometryEngineId> {
        self.entity_to_geometry
            .get(&entity_id)
            .map(|entry| *entry.value())
    }

    /// Get entity ID from geometry ID
    pub fn get_entity_id(&self, geometry_id: &GeometryEngineId) -> Option<EntityId> {
        let key = match geometry_id {
            GeometryEngineId::Solid(id) => format!("solid_{}", id),
            GeometryEngineId::Face(id) => format!("face_{}", id),
            GeometryEngineId::Edge(id) => format!("edge_{}", id),
            GeometryEngineId::Vertex(id) => format!("vertex_{}", id),
        };
        self.geometry_to_entity
            .get(&key)
            .map(|entry| *entry.value())
    }

    /// Get the parent solid for an entity
    pub fn get_parent_solid(&self, entity_id: EntityId) -> Option<SolidId> {
        self.entity_parent_solid
            .get(&entity_id)
            .map(|entry| *entry.value())
    }

    /// Remove a mapping
    pub fn remove(&self, entity_id: EntityId) {
        if let Some((_, geom_id)) = self.entity_to_geometry.remove(&entity_id) {
            let key = match geom_id {
                GeometryEngineId::Solid(id) => format!("solid_{}", id),
                GeometryEngineId::Face(id) => format!("face_{}", id),
                GeometryEngineId::Edge(id) => format!("edge_{}", id),
                GeometryEngineId::Vertex(id) => format!("vertex_{}", id),
            };
            self.geometry_to_entity.remove(&key);
        }
        self.entity_parent_solid.remove(&entity_id);
    }

    /// Clear all mappings
    pub fn clear(&self) {
        self.entity_to_geometry.clear();
        self.geometry_to_entity.clear();
        self.entity_parent_solid.clear();
    }

    /// Get total number of mappings
    pub fn len(&self) -> usize {
        self.entity_to_geometry.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.entity_to_geometry.is_empty()
    }
}

impl Default for EntityMapping {
    fn default() -> Self {
        Self::new()
    }
}

/// Global entity mapping instance
static ENTITY_MAPPING: once_cell::sync::Lazy<EntityMapping> =
    once_cell::sync::Lazy::new(EntityMapping::new);

/// Get the global entity mapping
pub fn get_entity_mapping() -> &'static EntityMapping {
    &ENTITY_MAPPING
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_solid_mapping() {
        let mapping = EntityMapping::new();
        let entity_id = EntityId::new();
        let solid_id = 42;

        mapping.register_solid(entity_id, solid_id);

        assert_eq!(
            mapping.get_geometry_id(entity_id),
            Some(GeometryEngineId::Solid(solid_id))
        );

        assert_eq!(
            mapping.get_entity_id(&GeometryEngineId::Solid(solid_id)),
            Some(entity_id)
        );
    }

    #[test]
    fn test_face_with_parent() {
        let mapping = EntityMapping::new();
        let entity_id = EntityId::new();
        let face_id = 10;
        let parent_solid = 5;

        mapping.register_face(entity_id, face_id, parent_solid);

        assert_eq!(
            mapping.get_geometry_id(entity_id),
            Some(GeometryEngineId::Face(face_id))
        );

        assert_eq!(mapping.get_parent_solid(entity_id), Some(parent_solid));
    }

    #[test]
    fn test_remove_mapping() {
        let mapping = EntityMapping::new();
        let entity_id = EntityId::new();
        let solid_id = 99;

        mapping.register_solid(entity_id, solid_id);
        assert!(!mapping.is_empty());

        mapping.remove(entity_id);
        assert!(mapping.get_geometry_id(entity_id).is_none());
        assert!(mapping
            .get_entity_id(&GeometryEngineId::Solid(solid_id))
            .is_none());
    }
}
