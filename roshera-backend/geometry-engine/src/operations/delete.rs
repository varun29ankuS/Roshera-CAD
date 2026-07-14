//! Delete Operations for B-Rep Models
//!
//! Provides operations to delete geometry entities with proper topology management,
//! cascade deletion, and orphan cleanup.

use super::lifecycle::{self, OpSpec};
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
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    }
    lifecycle::with_rollback(model, move |model| {
        delete_entities_body(model, target, options)
    })
}

fn delete_entities_body(
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
    lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    lifecycle::with_rollback(model, move |model| {
        delete_solid_body(model, solid_id, cascade)
    })
}

fn delete_solid_body(
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

    // Sweep unattributed debris. Cascading the shells above frees this solid's
    // own loops/edges/vertices, but deleting a part must ALSO purge any
    // pre-existing orphan topology a broken boolean left in the stores (faces
    // owned by no solid — the debris the model-level `model_debris_orphan_faces`
    // accounting reports). `clear_parts` used to be the only thing that purged
    // it; now `delete_part` does too, so an agent can clear a broken manifold's
    // fallout without wiping the whole model. Runs inside this `with_rollback`
    // closure, so a failure restores everything via the snapshot.
    prune_boolean_orphan_topology(model)?;

    Ok(deleted)
}

/// Delete a single face
pub fn delete_face(model: &mut BRepModel, face_id: FaceId, heal: bool) -> OperationResult<()> {
    lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    lifecycle::with_rollback(model, move |model| delete_face_body(model, face_id, heal))
}

#[allow(clippy::expect_used)] // face_id validated non-None at fn entry; not removed since
fn delete_face_body(model: &mut BRepModel, face_id: FaceId, heal: bool) -> OperationResult<()> {
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
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    }
    lifecycle::with_rollback(model, move |model| {
        delete_edge_body(model, edge_id, options)
    })
}

fn delete_edge_body(
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

/// Prune the orphan topology a boolean leaves after retiring its operand
/// `Solid` records.
///
/// A boolean builds its result as brand-new faces/loops/shells and a fresh
/// `Solid`, then removes only the operand `Solid` records — the operands'
/// husk shells, their faces, loops, edges and vertices all remain in the
/// stores. The result re-references the operands' SHARED edges and vertices
/// (and their surfaces), so those must survive; but the husk shells, and the
/// operand-only faces/loops/edges/vertices only they reach, are now
/// unreachable from any live solid. Left in place they are orphan topology:
/// `find_parent_solid` returns `None` for such a face, and its FaceId-keyed
/// sidecars dangle.
///
/// This runs the existing reference-counted GC ([`find_orphaned_entities`])
/// to a fixed point on the post-boolean model. An orphaned husk shell frees
/// the faces only it referenced, freeing their loops, then edges, then
/// vertices — each stratum re-tested against the LIVE result topology via
/// `is_*_used`, so every entity the result still references is retained and
/// only the operand husks drop. Depth is bounded (shell→face→loop→edge→vertex
/// = 5 levels); we cap at 6 passes defensively, matching `heal_deletion_gaps`.
///
/// For every dropped face the FaceId-keyed sidecars (`cap_apex_hint`, and the
/// PID maps `face_pids`/`pid_to_face`) are purged so no stale key dangles.
/// Removing an orphan cutter face that legitimately has no sidecar entry is a
/// no-op; the purge is defensive and correct. `gdt`/`labels`/`drf` are keyed
/// by `PersistentId`/`SolidId`, not `FaceId`, so a face drop does not key them
/// directly; the face's PID entry (which anchors any such annotation) is
/// removed with the face.
pub(crate) fn prune_boolean_orphan_topology(model: &mut BRepModel) -> OperationResult<()> {
    for _ in 0..6 {
        let orphans = find_orphaned_entities(model);
        if orphans.is_empty() {
            break;
        }
        for (entity_type, entity_id) in orphans {
            if entity_type == EntityType::Face {
                purge_face_sidecars(model, entity_id);
            }
            delete_entity(model, entity_type, entity_id)?;
        }
    }
    Ok(())
}

/// Remove every FaceId-keyed sidecar entry for a face being dropped, so no
/// dangling key survives its removal. `cap_apex_hint` is a direct
/// `FaceId`-keyed map; the PID maps are cleaned in lock-step.
///
/// The forward `face_pids[face]` entry is always the dead face's own and is
/// removed. The inverse `pid_to_face[pid]` is only removed when it STILL
/// points at this face: a boolean passthrough result face INHERITS its
/// parent operand face's PID (`assign_boolean_face_pids`), so after the
/// result face is minted `pid_to_face[pid]` points at the LIVE result face
/// while the retired parent still carries `face_pids[parent] = pid`. Blindly
/// removing the inverse would clobber the live result face's round-trip;
/// guarding on identity leaves the inherited mapping intact.
fn purge_face_sidecars(model: &mut BRepModel, face_id: FaceId) {
    model.cap_apex_hint.remove(&face_id);
    if let Some(pid) = model.face_pids.remove(&face_id) {
        if model.pid_to_face.get(&pid) == Some(&face_id) {
            model.pid_to_face.remove(&pid);
        }
    }
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

/// Iteratively remove orphaned topology entities until a fixed point is reached.
///
/// After primary deletion + cascade + single-pass orphan cleanup, removing one
/// orphan can create new orphans (e.g. deleting an orphan face frees its loops;
/// freeing the loops frees their edges; freeing the edges frees their vertices).
/// A single pass therefore leaves a stratum of dangling entities. We iterate
/// `find_orphaned_entities` to a fixed point — at most O(depth) passes, where
/// depth ≤ 5 (vertex→edge→loop→face→shell), so worst case is 5 sweeps.
///
/// Returns the cumulative list of entities healed away.
fn heal_deletion_gaps(model: &mut BRepModel) -> OperationResult<Vec<(EntityType, u32)>> {
    let mut healed = Vec::new();
    // Cap iterations defensively at the topology depth (5 levels). In practice
    // the loop terminates after 1-2 iterations once orphans run out.
    for _ in 0..6 {
        let orphans = find_orphaned_entities(model);
        if orphans.is_empty() {
            break;
        }
        for (entity_type, entity_id) in orphans {
            delete_entity(model, entity_type, entity_id)?;
            healed.push((entity_type, entity_id));
        }
    }
    Ok(healed)
}

/// Heal topology after a single face deletion.
///
/// At call time, the face and its loops have already been removed by the caller
/// (`delete_face`). Any edges that were used only by this face's loops are now
/// dangling, and the vertices at their endpoints may also have become orphans.
/// We delegate to `heal_deletion_gaps`, which performs transitive cleanup.
fn heal_face_deletion(model: &mut BRepModel, _face_id: FaceId) -> OperationResult<()> {
    // Transitive orphan cleanup handles edges and vertices freed by the
    // face's removed loops in arbitrary depth.
    let _ = heal_deletion_gaps(model)?;
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

/// Validate B-Rep referential integrity after a deletion.
///
/// A delete must leave the topology referentially consistent: every ID
/// referenced by a surviving entity must resolve to an existing entity in the
/// corresponding store. Dangling references are silent corruption — they
/// cause downstream operations (tessellation, boolean, export) to fail in
/// confusing places far from the original delete site.
///
/// This pass walks every survivor and asserts:
///   - Edge.start_vertex / end_vertex exist in `model.vertices`
///   - Edge.curve_id exists in `model.curves`
///   - Loop.edges all exist in `model.edges`
///   - Face.outer_loop and inner_loops all exist in `model.loops`
///   - Face.surface_id exists in `model.surfaces`
///   - Shell.faces all exist in `model.faces`
///   - Solid.outer_shell and inner_shells all exist in `model.shells`
///
/// Returns `OperationError::TopologyError` with the first dangling reference
/// found. The caller can then choose to roll back, repair, or surface to the
/// user.
fn validate_model_after_deletion(model: &BRepModel) -> OperationResult<()> {
    fn dangling(kind: &str, id: u32, ref_kind: &str) -> OperationError {
        OperationError::TopologyError(format!("{kind} {id} references missing {ref_kind}"))
    }

    // Edges → vertices, curves
    for (edge_id, edge) in model.edges.iter() {
        if model.vertices.get(edge.start_vertex).is_none() {
            return Err(dangling("Edge", edge_id, "start vertex"));
        }
        if model.vertices.get(edge.end_vertex).is_none() {
            return Err(dangling("Edge", edge_id, "end vertex"));
        }
        if model.curves.get(edge.curve_id).is_none() {
            return Err(dangling("Edge", edge_id, "curve"));
        }
    }

    // Loops → edges
    for (loop_id, lp) in model.loops.iter() {
        for &eid in &lp.edges {
            if model.edges.get(eid).is_none() {
                return Err(dangling("Loop", loop_id, "edge"));
            }
        }
    }

    // Faces → loops, surfaces
    for (face_id, face) in model.faces.iter() {
        if model.loops.get(face.outer_loop).is_none() {
            return Err(dangling("Face", face_id, "outer loop"));
        }
        for &lid in &face.inner_loops {
            if model.loops.get(lid).is_none() {
                return Err(dangling("Face", face_id, "inner loop"));
            }
        }
        if model.surfaces.get(face.surface_id).is_none() {
            return Err(dangling("Face", face_id, "surface"));
        }
    }

    // Shells → faces
    for (shell_id, shell) in model.shells.iter() {
        for &fid in &shell.faces {
            if model.faces.get(fid).is_none() {
                return Err(dangling("Shell", shell_id, "face"));
            }
        }
    }

    // Solids → shells
    for (solid_id, solid) in model.solids.iter() {
        if model.shells.get(solid.outer_shell).is_none() {
            return Err(dangling("Solid", solid_id, "outer shell"));
        }
        for &sid in &solid.inner_shells {
            if model.shells.get(sid).is_none() {
                return Err(dangling("Solid", solid_id, "inner shell"));
            }
        }
    }

    Ok(())
}
