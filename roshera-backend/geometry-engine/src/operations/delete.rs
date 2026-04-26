//! Delete Operations for B-Rep Models
//!
//! Provides operations to delete geometry entities with proper topology management,
//! cascade deletion, and orphan cleanup.

use super::{CommonOptions, OperationError, OperationResult};
use crate::primitives::{
    edge::EdgeId, face::FaceId, r#loop::LoopId, shell::ShellId, solid::SolidId,
    topology_builder::BRepModel, vertex::VertexId,
};
use std::collections::{HashSet, VecDeque};

/// Entity to delete
#[derive(Debug, Clone)]
pub enum DeleteTarget {
    /// Delete a solid and all its components
    Solid(SolidId),

    /// Delete a shell and its faces
    Shell(ShellId),

    /// Delete a face
    Face(FaceId),

    /// Delete a loop
    Loop(LoopId),

    /// Delete an edge
    Edge(EdgeId),

    /// Delete a vertex
    Vertex(VertexId),

    /// Delete multiple entities
    Multiple(Vec<DeleteTarget>),

    /// Delete all entities matching criteria
    ByFilter(DeleteFilter),
}

/// Filter for selective deletion
#[derive(Debug, Clone)]
pub struct DeleteFilter {
    /// Delete entities of specific types
    pub entity_types: Option<Vec<EntityType>>,

    /// Delete entities with specific IDs
    pub entity_ids: Option<Vec<u32>>,

    /// Delete entities within a region
    pub within_region: Option<BoundingRegion>,

    /// Delete entities with specific properties
    pub with_properties: Option<PropertyFilter>,
}

/// Entity types for deletion
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityType {
    Solid,
    Shell,
    Face,
    Loop,
    Edge,
    Vertex,
}

/// Bounding region for spatial deletion
#[derive(Debug, Clone)]
pub enum BoundingRegion {
    Box {
        min: [f64; 3],
        max: [f64; 3],
    },
    Sphere {
        center: [f64; 3],
        radius: f64,
    },
    Cylinder {
        axis: [f64; 3],
        point: [f64; 3],
        radius: f64,
        height: f64,
    },
}

/// Property filter for deletion
#[derive(Debug, Clone)]
pub struct PropertyFilter {
    pub name_pattern: Option<String>,
    pub material: Option<String>,
    pub color: Option<[f32; 4]>,
    pub visible: Option<bool>,
}

/// Options for delete operations
#[derive(Debug, Clone)]
pub struct DeleteOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Whether to cascade delete dependent entities
    pub cascade: bool,

    /// Whether to delete orphaned entities
    pub delete_orphans: bool,

    /// Whether to heal gaps after deletion
    pub heal_gaps: bool,

    /// Whether to validate topology after deletion
    pub validate_topology: bool,
}

impl Default for DeleteOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            cascade: true,
            delete_orphans: true,
            heal_gaps: false,
            validate_topology: true,
        }
    }
}

/// Result of a delete operation
#[derive(Debug)]
pub struct DeleteResult {
    /// Primary entities that were deleted
    pub deleted_primary: Vec<(EntityType, u32)>,

    /// Entities deleted due to cascade
    pub deleted_cascade: Vec<(EntityType, u32)>,

    /// Orphaned entities that were deleted
    pub deleted_orphans: Vec<(EntityType, u32)>,

    /// Entities that were healed/modified
    pub healed_entities: Vec<(EntityType, u32)>,

    /// Total number of entities deleted
    pub total_deleted: usize,
}

/// Delete entities from the model
pub fn delete_entities(
    model: &mut BRepModel,
    target: DeleteTarget,
    options: DeleteOptions,
) -> OperationResult<DeleteResult> {
    // Collect entities to delete
    let mut to_delete = collect_entities_to_delete(model, &target)?;

    // Track results
    let mut deleted_primary = Vec::new();
    let mut deleted_cascade = Vec::new();
    let mut deleted_orphans = Vec::new();
    let mut healed_entities = Vec::new();

    // Perform primary deletion
    for (entity_type, entity_id) in &to_delete {
        deleted_primary.push((*entity_type, *entity_id));
    }

    // Cascade deletion if enabled
    if options.cascade {
        let cascade_targets = find_cascade_targets(model, &to_delete);
        for (entity_type, entity_id) in cascade_targets {
            if !to_delete.contains(&(entity_type, entity_id)) {
                deleted_cascade.push((entity_type, entity_id));
                to_delete.insert((entity_type, entity_id));
            }
        }
    }

    // Delete the entities
    for (entity_type, entity_id) in &to_delete {
        delete_entity(model, *entity_type, *entity_id)?;
    }

    // Find and delete orphans if enabled
    if options.delete_orphans {
        let orphans = find_orphaned_entities(model);
        for (entity_type, entity_id) in orphans {
            delete_entity(model, entity_type, entity_id)?;
            deleted_orphans.push((entity_type, entity_id));
        }
    }

    // Heal gaps if enabled
    if options.heal_gaps {
        let healed = heal_deletion_gaps(model)?;
        healed_entities = healed;
    }

    // Validate topology if requested
    if options.validate_topology {
        validate_model_after_deletion(model)?;
    }

    let total_deleted = deleted_primary.len() + deleted_cascade.len() + deleted_orphans.len();

    Ok(DeleteResult {
        deleted_primary,
        deleted_cascade,
        deleted_orphans,
        healed_entities,
        total_deleted,
    })
}

/// Delete a single solid with all its components
pub fn delete_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    cascade: bool,
) -> OperationResult<Vec<(EntityType, u32)>> {
    let mut deleted = Vec::new();

    // Get the solid and clone necessary data
    let (outer_shell, inner_shells) = {
        let solid = model
            .solids
            .get(solid_id)
            .ok_or_else(|| OperationError::InvalidInput {
                parameter: "solid_id".to_string(),
                expected: "existing solid".to_string(),
                received: format!("{}", solid_id),
            })?;
        (solid.outer_shell, solid.inner_shells.clone())
    };

    // Delete shells if cascading
    if cascade {
        // Delete outer shell
        delete_shell_cascade(model, outer_shell, &mut deleted)?;

        // Delete inner shells
        for inner_shell in &inner_shells {
            delete_shell_cascade(model, *inner_shell, &mut deleted)?;
        }
    }

    // Remove the solid
    model.solids.remove(solid_id);
    deleted.push((EntityType::Solid, solid_id));

    Ok(deleted)
}

/// Delete a single face
#[allow(clippy::expect_used)] // face_id validated non-None at fn entry; not removed since
pub fn delete_face(model: &mut BRepModel, face_id: FaceId, heal: bool) -> OperationResult<()> {
    // Validate face exists
    if model.faces.get(face_id).is_none() {
        return Err(OperationError::InvalidInput {
            parameter: "face_id".to_string(),
            expected: "existing face".to_string(),
            received: format!("{}", face_id),
        });
    }

    // Find shells using this face
    let mut affected_shells = Vec::new();
    for (shell_id, shell) in model.shells.iter() {
        if shell.faces.contains(&face_id) {
            affected_shells.push(shell_id);
        }
    }

    // Remove face from shells
    for shell_id in affected_shells {
        if let Some(shell) = model.shells.get_mut(shell_id) {
            shell.remove_face(face_id);
        }
    }

    // Get face loops before deletion. `face_id` was validated at the
    // top of this function via `is_none()` -> early return; the face
    // store has not been mutated to remove it since.
    let face = model
        .faces
        .get(face_id)
        .expect("face_id validated non-None above and not yet removed");
    let outer_loop = face.outer_loop;
    let inner_loops = face.inner_loops.to_vec();

    // Delete the face
    model.faces.remove(face_id);

    // Delete associated loops
    model.loops.remove(outer_loop);
    for loop_id in inner_loops {
        model.loops.remove(loop_id);
    }

    // Heal if requested
    if heal {
        heal_face_deletion(model, face_id)?;
    }

    Ok(())
}

/// Delete an edge
pub fn delete_edge(
    model: &mut BRepModel,
    edge_id: EdgeId,
    options: DeleteOptions,
) -> OperationResult<DeleteResult> {
    let deleted_primary = vec![(EntityType::Edge, edge_id)];
    let mut deleted_cascade = Vec::new();
    let mut deleted_orphans = Vec::new();
    let healed_entities = Vec::new();

    // Find loops using this edge
    let mut affected_loops = Vec::new();
    for (loop_id, loop_) in model.loops.iter() {
        if loop_.edges.contains(&edge_id) {
            affected_loops.push(loop_id);
        }
    }

    // Handle affected loops
    for loop_id in affected_loops {
        if options.cascade {
            // Delete the entire loop
            model.loops.remove(loop_id);
            deleted_cascade.push((EntityType::Loop, loop_id));

            // Find and handle affected faces
            for (face_id, face) in model.faces.iter() {
                if face.outer_loop == loop_id || face.inner_loops.contains(&loop_id) {
                    deleted_cascade.push((EntityType::Face, face_id));
                }
            }
        } else {
            // Remove edge from loop
            if let Some(loop_) = model.loops.get_mut(loop_id) {
                // Find the index of the edge in the loop
                if let Some(idx) = loop_.edges.iter().position(|&e| e == edge_id) {
                    loop_.remove_edge(idx);
                }
            }
        }
    }

    // Get edge data before deletion
    let (start_vertex, end_vertex) = if options.delete_orphans {
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidInput {
                parameter: "edge_id".to_string(),
                expected: "existing edge".to_string(),
                received: format!("{}", edge_id),
            })?;
        (edge.start_vertex, edge.end_vertex)
    } else {
        (0, 0) // Dummy values if not checking orphans
    };

    // Delete the edge
    model.edges.remove(edge_id);

    // Find orphaned vertices
    if options.delete_orphans {
        // Check if vertices are now orphaned
        if !is_vertex_used(model, start_vertex) {
            model.vertices.remove(start_vertex);
            deleted_orphans.push((EntityType::Vertex, start_vertex));
        }
        if !is_vertex_used(model, end_vertex) {
            model.vertices.remove(end_vertex);
            deleted_orphans.push((EntityType::Vertex, end_vertex));
        }
    }

    let total_deleted = deleted_primary.len() + deleted_cascade.len() + deleted_orphans.len();

    Ok(DeleteResult {
        deleted_primary,
        deleted_cascade,
        deleted_orphans,
        healed_entities,
        total_deleted,
    })
}

// Helper functions

fn collect_entities_to_delete(
    model: &BRepModel,
    target: &DeleteTarget,
) -> OperationResult<HashSet<(EntityType, u32)>> {
    let mut entities = HashSet::new();

    match target {
        DeleteTarget::Solid(id) => {
            entities.insert((EntityType::Solid, *id));
        }
        DeleteTarget::Shell(id) => {
            entities.insert((EntityType::Shell, *id));
        }
        DeleteTarget::Face(id) => {
            entities.insert((EntityType::Face, *id));
        }
        DeleteTarget::Loop(id) => {
            entities.insert((EntityType::Loop, *id));
        }
        DeleteTarget::Edge(id) => {
            entities.insert((EntityType::Edge, *id));
        }
        DeleteTarget::Vertex(id) => {
            entities.insert((EntityType::Vertex, *id));
        }
        DeleteTarget::Multiple(targets) => {
            for sub_target in targets {
                let sub_entities = collect_entities_to_delete(model, sub_target)?;
                entities.extend(sub_entities);
            }
        }
        DeleteTarget::ByFilter(filter) => {
            let filtered = apply_delete_filter(model, filter);
            entities.extend(filtered);
        }
    }

    Ok(entities)
}

fn find_cascade_targets(
    model: &BRepModel,
    initial: &HashSet<(EntityType, u32)>,
) -> HashSet<(EntityType, u32)> {
    let mut cascade = HashSet::new();
    let mut queue = VecDeque::from_iter(initial.iter().cloned());

    while let Some((entity_type, entity_id)) = queue.pop_front() {
        match entity_type {
            EntityType::Solid => {
                // Cascade to shells
                if let Some(solid) = model.solids.get(entity_id) {
                    cascade.insert((EntityType::Shell, solid.outer_shell));
                    for inner in &solid.inner_shells {
                        cascade.insert((EntityType::Shell, *inner));
                    }
                }
            }
            EntityType::Shell => {
                // Cascade to faces
                if let Some(shell) = model.shells.get(entity_id) {
                    for face in &shell.faces {
                        cascade.insert((EntityType::Face, *face));
                    }
                }
            }
            EntityType::Face => {
                // Cascade to loops
                if let Some(face) = model.faces.get(entity_id) {
                    cascade.insert((EntityType::Loop, face.outer_loop));
                    for inner in &face.inner_loops {
                        cascade.insert((EntityType::Loop, *inner));
                    }
                }
            }
            EntityType::Loop => {
                // Loops contain edges but don't own them
            }
            EntityType::Edge => {
                // Edges reference vertices but don't own them
            }
            EntityType::Vertex => {
                // Vertices are leaf entities
            }
        }
    }

    cascade
}

fn find_orphaned_entities(model: &BRepModel) -> Vec<(EntityType, u32)> {
    let mut orphans = Vec::new();

    // Find orphaned vertices
    for (vertex_id, _) in model.vertices.iter() {
        if !is_vertex_used(model, vertex_id) {
            orphans.push((EntityType::Vertex, vertex_id));
        }
    }

    // Find orphaned edges
    for (edge_id, _) in model.edges.iter() {
        if !is_edge_used(model, edge_id) {
            orphans.push((EntityType::Edge, edge_id));
        }
    }

    // Find orphaned loops
    for (loop_id, _) in model.loops.iter() {
        if !is_loop_used(model, loop_id) {
            orphans.push((EntityType::Loop, loop_id));
        }
    }

    // Find orphaned faces
    for (face_id, _) in model.faces.iter() {
        if !is_face_used(model, face_id) {
            orphans.push((EntityType::Face, face_id));
        }
    }

    // Find orphaned shells
    for (shell_id, _) in model.shells.iter() {
        if !is_shell_used(model, shell_id) {
            orphans.push((EntityType::Shell, shell_id));
        }
    }

    orphans
}

fn delete_entity(
    model: &mut BRepModel,
    entity_type: EntityType,
    entity_id: u32,
) -> OperationResult<()> {
    match entity_type {
        EntityType::Solid => {
            model.solids.remove(entity_id);
        }
        EntityType::Shell => {
            model.shells.remove(entity_id);
        }
        EntityType::Face => {
            model.faces.remove(entity_id);
        }
        EntityType::Loop => {
            model.loops.remove(entity_id);
        }
        EntityType::Edge => {
            model.edges.remove(entity_id);
        }
        EntityType::Vertex => {
            model.vertices.remove(entity_id);
        }
    }
    Ok(())
}

fn delete_shell_cascade(
    model: &mut BRepModel,
    shell_id: ShellId,
    deleted: &mut Vec<(EntityType, u32)>,
) -> OperationResult<()> {
    if let Some(shell) = model.shells.get(shell_id) {
        // Delete all faces in shell
        for face_id in &shell.faces {
            model.faces.remove(*face_id);
            deleted.push((EntityType::Face, *face_id));
        }

        // Delete the shell
        model.shells.remove(shell_id);
        deleted.push((EntityType::Shell, shell_id));
    }
    Ok(())
}

fn heal_deletion_gaps(_model: &mut BRepModel) -> OperationResult<Vec<(EntityType, u32)>> {
    // This would implement gap healing algorithms
    // For now, return empty
    Ok(Vec::new())
}

fn heal_face_deletion(_model: &mut BRepModel, _face_id: FaceId) -> OperationResult<()> {
    // This would implement face deletion healing
    // Could involve extending adjacent faces, etc.
    Ok(())
}

fn apply_delete_filter(model: &BRepModel, filter: &DeleteFilter) -> HashSet<(EntityType, u32)> {
    let mut result = HashSet::new();

    // Apply entity type filter
    if let Some(ref types) = filter.entity_types {
        for entity_type in types {
            match entity_type {
                EntityType::Solid => {
                    for (id, _) in model.solids.iter() {
                        result.insert((*entity_type, id));
                    }
                }
                EntityType::Shell => {
                    for (id, _) in model.shells.iter() {
                        result.insert((*entity_type, id));
                    }
                }
                EntityType::Face => {
                    for (id, _) in model.faces.iter() {
                        result.insert((*entity_type, id));
                    }
                }
                EntityType::Loop => {
                    for (id, _) in model.loops.iter() {
                        result.insert((*entity_type, id));
                    }
                }
                EntityType::Edge => {
                    for (id, _) in model.edges.iter() {
                        result.insert((*entity_type, id));
                    }
                }
                EntityType::Vertex => {
                    for (id, _) in model.vertices.iter() {
                        result.insert((*entity_type, id));
                    }
                }
            }
        }
    }

    // Apply other filters...

    result
}

fn is_vertex_used(model: &BRepModel, vertex_id: VertexId) -> bool {
    for (_, edge) in model.edges.iter() {
        if edge.start_vertex == vertex_id || edge.end_vertex == vertex_id {
            return true;
        }
    }
    false
}

fn is_edge_used(model: &BRepModel, edge_id: EdgeId) -> bool {
    for (_, loop_) in model.loops.iter() {
        if loop_.edges.contains(&edge_id) {
            return true;
        }
    }
    false
}

fn is_loop_used(model: &BRepModel, loop_id: LoopId) -> bool {
    for (_, face) in model.faces.iter() {
        if face.outer_loop == loop_id || face.inner_loops.contains(&loop_id) {
            return true;
        }
    }
    false
}

fn is_face_used(model: &BRepModel, face_id: FaceId) -> bool {
    for (_, shell) in model.shells.iter() {
        if shell.faces.contains(&face_id) {
            return true;
        }
    }
    false
}

fn is_shell_used(model: &BRepModel, shell_id: ShellId) -> bool {
    for (_, solid) in model.solids.iter() {
        if solid.outer_shell == shell_id || solid.inner_shells.contains(&shell_id) {
            return true;
        }
    }
    false
}

fn validate_model_after_deletion(_model: &BRepModel) -> OperationResult<()> {
    // Validate topology integrity
    // Check for dangling references
    // Validate Euler characteristics
    Ok(())
}

