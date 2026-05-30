//! Transform Operations for B-Rep Models
//!
//! Applies transformations (translate, rotate, scale, mirror) to B-Rep entities
//! while maintaining topological integrity and analytical precision.
//!
//! Indexed access into matrix rows / point coordinate arrays is the canonical
//! idiom for affine transformation — bounded by 4x4 matrix and 3D vector
//! constants. Matches the pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use super::lifecycle::{self, OpSpec};
use super::{CommonOptions, OperationError, OperationResult};
use crate::math::{Matrix4, Point3, Vector3};
use crate::primitives::{
    edge::EdgeId, face::FaceId, solid::SolidId, topology_builder::BRepModel, vertex::VertexId,
};
use std::collections::{HashMap, HashSet};

/// Options for transform operations
#[derive(Debug, Clone)]
pub struct TransformOptions {
    /// Common operation options
    pub common: CommonOptions,

    /// Whether to update surface parameterization
    pub update_parameterization: bool,
}

impl Default for TransformOptions {
    fn default() -> Self {
        Self {
            common: CommonOptions::default(),
            update_parameterization: true,
        }
    }
}

/// Transform result
#[derive(Debug)]
pub struct TransformResult {
    /// Transformed entities (transforms apply in place; callers wanting a
    /// duplicate must clone the underlying solid prior to invocation).
    pub transformed_ids: Vec<u32>,
    /// Transform matrix applied
    pub transform: Matrix4,
}

/// Apply transformation to a solid
pub fn transform_solid(
    model: &mut BRepModel,
    solid_id: SolidId,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    }
    lifecycle::with_rollback(model, move |model| {
        transform_solid_body(model, solid_id, transform, options)
    })
}

fn transform_solid_body(
    model: &mut BRepModel,
    solid_id: SolidId,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Validate inputs
    validate_transform_inputs(model, &transform)?;

    let solid = solid_id;

    // Get all entities in solid
    let mut entities = get_solid_entities(model, solid)?;

    // Detach this solid's vertices from any other solid that happens to
    // share them. `VertexStore::add_or_find` is the canonical primitive-
    // construction primitive and deduplicates coincident positions; two
    // primitives built at the same coordinates (e.g. two `create_box_3d`
    // calls at the origin) silently share their corner vertices. An
    // in-place transform would then mutate the foreign solid's geometry
    // — see `tests/spatial_broad_phase_pruning.rs::disjoint_unit_boxes_*`
    // for the regression that exposed this. Cloning the shared vertices
    // (and rewriting this solid's edge endpoints to reference the
    // clones) contains the transform to the target topology.
    isolate_shared_topology(model, solid, &mut entities)?;

    // Transform vertices
    transform_vertices(model, &entities.vertices, &transform)?;

    // Transform curves
    transform_curves(model, &entities.edges, &transform)?;

    // Transform surfaces
    if options.update_parameterization {
        transform_surfaces(model, &entities.faces, &transform)?;
    }

    // Validate result
    if options.common.validate_result {
        validate_transformed_solid(model, solid)?;
    }

    // Slice 5: vertices moved → solid bbox changed → location
    // descriptor stale.
    model.location_cache.invalidate(solid_id);

    // Vertices moved → cached volume/COM/inertia on the Solid are stale.
    // Same contract as fillet_edges / chamfer_edges / extrude_face.
    if let Some(solid) = model.solids.get_mut(solid_id) {
        solid.invalidate_mass_props_cache();
    }

    // Record the operation for timeline / event-sourcing consumers.
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("transform_solid")
            .with_parameters(serde_json::json!({
                "solid_id": solid_id,
                "transform": transform,
                "update_parameterization": options.update_parameterization,
            }))
            .with_input_solids([solid_id as u64])
            .with_output_solids([solid as u64]),
    );

    Ok(TransformResult {
        transformed_ids: vec![solid],
        transform,
    })
}

/// Apply transformation to faces
pub fn transform_faces(
    model: &mut BRepModel,
    face_ids: Vec<FaceId>,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    }
    lifecycle::with_rollback(model, move |model| {
        transform_faces_body(model, face_ids, transform, options)
    })
}

fn transform_faces_body(
    model: &mut BRepModel,
    face_ids: Vec<FaceId>,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    validate_transform_inputs(model, &transform)?;

    let input_face_ids: Vec<u32> = face_ids.clone();

    let faces = face_ids.clone();

    // Get all entities used by faces
    let entities = get_faces_entities(model, &faces)?;

    // Transform vertices
    transform_vertices(model, &entities.vertices, &transform)?;

    // Transform curves
    transform_curves(model, &entities.edges, &transform)?;

    // Transform surfaces
    if options.update_parameterization {
        transform_surfaces(model, &faces, &transform)?;
    }

    let output_face_ids: Vec<u32> = faces.clone();
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("transform_faces")
            .with_parameters(serde_json::json!({
                "transform": transform,
                "update_parameterization": options.update_parameterization,
            }))
            .with_input_faces(input_face_ids.iter().map(|&f| f as u64))
            .with_output_faces(output_face_ids.iter().map(|&f| f as u64)),
    );

    Ok(TransformResult {
        transformed_ids: faces.into_iter().map(|f| f).collect(),
        transform,
    })
}

/// Apply transformation to edges
pub fn transform_edges(
    model: &mut BRepModel,
    edge_ids: Vec<EdgeId>,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    }
    lifecycle::with_rollback(model, move |model| {
        transform_edges_body(model, edge_ids, transform, options)
    })
}

fn transform_edges_body(
    model: &mut BRepModel,
    edge_ids: Vec<EdgeId>,
    transform: Matrix4,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    validate_transform_inputs(model, &transform)?;

    let input_edge_ids: Vec<u32> = edge_ids.clone();

    let edges = edge_ids.clone();

    // Get vertices used by edges
    let mut vertices = HashSet::new();
    for &edge_id in &edges {
        if let Some(edge) = model.edges.get(edge_id) {
            vertices.insert(edge.start_vertex);
            vertices.insert(edge.end_vertex);
        }
    }

    // Transform vertices
    let vertex_vec: Vec<_> = vertices.into_iter().collect();
    transform_vertices(model, &vertex_vec, &transform)?;

    // Transform curves
    transform_curves(model, &edges, &transform)?;

    let output_edge_ids: Vec<u32> = edges.clone();
    model.record_operation(
        crate::operations::recorder::RecordedOperation::new("transform_edges")
            .with_parameters(serde_json::json!({
                "transform": transform,
                "update_parameterization": options.update_parameterization,
            }))
            .with_input_edges(input_edge_ids.iter().map(|&e| e as u64))
            .with_output_edges(output_edge_ids.iter().map(|&e| e as u64)),
    );

    Ok(TransformResult {
        transformed_ids: edges.into_iter().map(|e| e).collect(),
        transform,
    })
}

/// Translate entities
pub fn translate(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    direction: Vector3,
    distance: f64,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    let transform = Matrix4::from_translation(&(direction * distance));

    // Dispatch based on entity type
    // For simplicity, assuming solids
    transform_solid(model, entity_ids[0], transform, options)
}

/// Rotate entities
pub fn rotate(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    axis_origin: Point3,
    axis_direction: Vector3,
    angle: f64,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Build rotation matrix
    let transform = Matrix4::rotation_axis(axis_origin, axis_direction, angle)?;

    // Dispatch based on entity type
    transform_solid(model, entity_ids[0], transform, options)
}

/// Scale entities
pub fn scale(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    scale_origin: Point3,
    scale_factors: Vector3,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    // Validate scale factors
    if scale_factors.x <= 0.0 || scale_factors.y <= 0.0 || scale_factors.z <= 0.0 {
        return Err(OperationError::InvalidGeometry(
            "Scale factors must be positive".to_string(),
        ));
    }

    // Build scale matrix
    let transform = Matrix4::scale_about_point(scale_origin, scale_factors);

    // Dispatch based on entity type
    transform_solid(model, entity_ids[0], transform, options)
}

/// Mirror entities
pub fn mirror(
    model: &mut BRepModel,
    entity_ids: Vec<u32>,
    plane_origin: Point3,
    plane_normal: Vector3,
    options: TransformOptions,
) -> OperationResult<TransformResult> {
    if options.common.validate_before {
        lifecycle::validate_can_apply(model, OpSpec::Generic)?;
    }
    lifecycle::with_rollback(model, move |model| {
        // Build mirror matrix
        let transform = Matrix4::mirror(plane_origin, plane_normal)?;

        // Dispatch based on entity type. The inner `transform_solid`
        // takes its own snapshot; we accept the nested-snapshot cost
        // for transactional correctness across the combined
        // mirror+orient-fix path.
        let result = transform_solid(model, entity_ids[0], transform, options)?;

        // Mirroring reverses orientation, need to fix
        fix_mirrored_orientations(model, result.transformed_ids[0])?;

        Ok(result)
    })
}

/// Transform vertices in place.
///
/// Earlier this routine called `model.vertices.add(...)` for each vertex,
/// which appended a *new* vertex with the transformed position while
/// leaving every edge / loop / face still pointing at the original
/// (untransformed) vertices. Net effect: callers like `translate` /
/// `rotate` / `scale` returned `Ok(...)` while the model was visually
/// unchanged. Mutating in place via `VertexStore::set_position` keeps
/// every existing topology reference valid and actually moves the solid.
fn transform_vertices(
    model: &mut BRepModel,
    vertex_ids: &[VertexId],
    transform: &Matrix4,
) -> OperationResult<Vec<VertexId>> {
    for &vertex_id in vertex_ids {
        let pos = match model.vertices.get(vertex_id) {
            Some(v) => Point3::from(v.position),
            None => {
                return Err(OperationError::InvalidGeometry(
                    "Vertex not found".to_string(),
                ));
            }
        };
        let transformed = transform.transform_point(&pos);
        if !model
            .vertices
            .set_position(vertex_id, transformed.x, transformed.y, transformed.z)
        {
            return Err(OperationError::InvalidGeometry(format!(
                "Failed to update vertex {vertex_id}"
            )));
        }
    }
    Ok(vertex_ids.to_vec())
}

/// Transform curves
fn transform_curves(
    model: &mut BRepModel,
    edge_ids: &[EdgeId],
    transform: &Matrix4,
) -> OperationResult<()> {
    // Collect the set of distinct curve IDs referenced by the edges first, so we
    // do not alias `model.edges` and `model.curves` mutably at the same time.
    let mut curve_ids: Vec<_> = edge_ids
        .iter()
        .filter_map(|&edge_id| model.edges.get(edge_id).map(|e| e.curve_id))
        .collect();
    curve_ids.sort_unstable();
    curve_ids.dedup();

    for curve_id in curve_ids {
        // Swap the curve in-place for its transformed image. Since `Curve::transform`
        // returns a fresh `Box<dyn Curve>`, we can replace the slot directly without
        // invalidating edge references (edges keep pointing to the same CurveId).
        if let Some(slot) = model.curves.get_mut(curve_id) {
            let transformed = slot.transform(transform);
            *slot = transformed;
        }
    }

    Ok(())
}

/// Transform surfaces
fn transform_surfaces(
    model: &mut BRepModel,
    face_ids: &[FaceId],
    transform: &Matrix4,
) -> OperationResult<()> {
    // Collect the distinct surface IDs up front to avoid holding a reference
    // into `model.faces` while we mutate `model.surfaces`.
    let mut surface_ids: Vec<_> = face_ids
        .iter()
        .filter_map(|&face_id| model.faces.get(face_id).map(|f| f.surface_id))
        .collect();
    surface_ids.sort_unstable();
    surface_ids.dedup();

    for surface_id in surface_ids {
        // Build the transformed surface from the current one and swap it in
        // place so face references stay valid.
        let Some(current) = model.surfaces.get(surface_id) else {
            continue;
        };
        let transformed = current.transform(transform);
        if model.surfaces.replace(surface_id, transformed).is_none() {
            return Err(OperationError::InvalidGeometry(format!(
                "transform_surfaces: surface {surface_id} not found in store"
            )));
        }
    }

    Ok(())
}

/// Detach a target solid's vertices from any other solid that shares them.
///
/// Primitive constructors (`create_box_3d`, `create_cylinder_3d`, polygon
/// builders, …) deduplicate coincident vertex positions through
/// [`VertexStore::add_or_find`]. The dedup is correct within a single
/// primitive — a polygon's closing edge must reuse the start vertex
/// rather than introducing a hairline gap — but it spans primitives,
/// so two coincidentally-placed builds (e.g. two boxes constructed at
/// the origin before one is translated away) silently share their
/// corner vertices.
///
/// An in-place `transform_solid` on either share-holder then walks
/// the shared vertex set via `get_solid_entities` and mutates the
/// positions, corrupting the foreign solid's loops. Symptoms surface
/// as misclassified faces in boolean ops, broken bbox queries, and
/// non-manifold output meshes.
///
/// The fix is to clone every shared vertex before the transform fires,
/// rewrite *this* solid's edge endpoints to point at the clones, and
/// update the entity snapshot so the downstream `transform_vertices`
/// call mutates only the cloned vertices. The foreign solid retains
/// the originals, untouched.
///
/// Edges and curves are not shared cross-primitive (each
/// `create_*` site calls `EdgeStore::add` / `CurveStore::add`, not
/// `add_or_find`), so they do not require an analogous pass.
fn isolate_shared_topology(
    model: &mut BRepModel,
    target_solid: SolidId,
    target_entities: &mut SolidEntities,
) -> OperationResult<()> {
    // Snapshot the IDs of every *other* solid up front so we can release
    // the immutable borrow on `model.solids` before re-borrowing through
    // `get_solid_entities`.
    let other_solid_ids: Vec<SolidId> = model
        .solids
        .iter()
        .filter_map(|(id, _)| (id != target_solid).then_some(id))
        .collect();

    if other_solid_ids.is_empty() {
        return Ok(());
    }

    let mut foreign_vertices: HashSet<VertexId> = HashSet::new();
    for other_id in other_solid_ids {
        // A foreign solid may itself have inconsistent topology (e.g.
        // a partly-built scratch solid mid-operation); tolerate that
        // by skipping rather than aborting the transform.
        if let Ok(other) = get_solid_entities(model, other_id) {
            foreign_vertices.extend(other.vertices);
        }
    }

    if foreign_vertices.is_empty() {
        return Ok(());
    }

    // Identify which of this solid's vertices are also referenced by
    // some foreign solid.
    let shared: Vec<VertexId> = target_entities
        .vertices
        .iter()
        .copied()
        .filter(|v| foreign_vertices.contains(v))
        .collect();

    if shared.is_empty() {
        return Ok(());
    }

    // Clone each shared vertex at its current position and build the
    // old → new remap.
    let mut remap: HashMap<VertexId, VertexId> = HashMap::with_capacity(shared.len());
    for old_id in shared {
        let pos = model.vertices.get_position(old_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "isolate_shared_topology: vertex {old_id} not found in store"
            ))
        })?;
        let new_id = model.vertices.add(pos[0], pos[1], pos[2]);
        remap.insert(old_id, new_id);
    }

    // Rewrite this solid's edge endpoints to reference the cloned vertices.
    for &edge_id in &target_entities.edges {
        let Some(edge) = model.edges.get_mut(edge_id) else {
            continue;
        };
        if let Some(&new_s) = remap.get(&edge.start_vertex) {
            edge.start_vertex = new_s;
        }
        if let Some(&new_e) = remap.get(&edge.end_vertex) {
            edge.end_vertex = new_e;
        }
    }

    // Update the entity snapshot so `transform_vertices` mutates the
    // clones rather than the originals (which now belong solely to the
    // foreign solid).
    for v in target_entities.vertices.iter_mut() {
        if let Some(&new_id) = remap.get(v) {
            *v = new_id;
        }
    }

    Ok(())
}

/// Get all entities in a solid
struct SolidEntities {
    vertices: Vec<VertexId>,
    edges: Vec<EdgeId>,
    faces: Vec<FaceId>,
}

fn get_solid_entities(model: &BRepModel, solid_id: SolidId) -> OperationResult<SolidEntities> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?;

    let shell = model
        .shells
        .get(solid.outer_shell)
        .ok_or_else(|| OperationError::InvalidGeometry("Shell not found".to_string()))?;

    let mut vertices = HashSet::new();
    let mut edges = HashSet::new();
    let faces = shell.faces.clone();

    // Collect every edge / vertex reachable through *all* loops of every
    // face — both the outer boundary and any interior (hole) loops. The
    // earlier implementation walked only `face.outer_loop`, so faces with
    // inner loops (e.g. an annular planar cap left by a boolean
    // difference) had their hole edges and vertices skipped. Transforming
    // such a solid would move the outer hull but leave the hole vertices
    // sitting at the original position, producing a torn / self-
    // intersecting model that broke downstream operations (booleans,
    // fillet-of-fillet, mass-property integration).
    for &face_id in &faces {
        if let Some(face) = model.faces.get(face_id) {
            for &loop_id in std::iter::once(&face.outer_loop).chain(face.inner_loops.iter()) {
                if let Some(loop_data) = model.loops.get(loop_id) {
                    for &edge_id in &loop_data.edges {
                        edges.insert(edge_id);

                        if let Some(edge) = model.edges.get(edge_id) {
                            vertices.insert(edge.start_vertex);
                            vertices.insert(edge.end_vertex);
                        }
                    }
                }
            }
        }
    }

    Ok(SolidEntities {
        vertices: vertices.into_iter().collect(),
        edges: edges.into_iter().collect(),
        faces,
    })
}

/// Get all entities used by faces
fn get_faces_entities(model: &BRepModel, face_ids: &[FaceId]) -> OperationResult<SolidEntities> {
    let mut vertices = HashSet::new();
    let mut edges = HashSet::new();

    // Walk every loop (outer + any inner / hole loops) on each face — see
    // the rationale in `get_solid_entities` for why inner loops must be
    // included.
    for &face_id in face_ids {
        if let Some(face) = model.faces.get(face_id) {
            for &loop_id in std::iter::once(&face.outer_loop).chain(face.inner_loops.iter()) {
                if let Some(loop_data) = model.loops.get(loop_id) {
                    for &edge_id in &loop_data.edges {
                        edges.insert(edge_id);

                        if let Some(edge) = model.edges.get(edge_id) {
                            vertices.insert(edge.start_vertex);
                            vertices.insert(edge.end_vertex);
                        }
                    }
                }
            }
        }
    }

    Ok(SolidEntities {
        vertices: vertices.into_iter().collect(),
        edges: edges.into_iter().collect(),
        faces: face_ids.to_vec(),
    })
}

/// Fix face / edge orientations after mirroring.
///
/// A reflection has determinant −1 and reverses the handedness of every
/// loop in the solid. Without flipping orientations, every face's
/// outward normal points inward and the solid is inside-out — booleans,
/// volume integration, and tessellation all silently produce the wrong
/// result. This walks every face in every shell of the solid, flips
/// each face's `FaceOrientation`, and flips every edge orientation
/// inside the face's outer + inner loops so the loop traversal still
/// agrees with the reversed face normal.
fn fix_mirrored_orientations(model: &mut BRepModel, solid_id: SolidId) -> OperationResult<()> {
    let solid = model
        .solids
        .get(solid_id)
        .ok_or_else(|| OperationError::InvalidGeometry("Solid not found".to_string()))?
        .clone();

    let shell_ids = solid.all_shells();

    // Collect all face IDs first so we can mutate faces without holding
    // an immutable borrow on shells.
    let mut face_ids: Vec<FaceId> = Vec::new();
    for shell_id in &shell_ids {
        if let Some(shell) = model.shells.get(*shell_id) {
            face_ids.extend(shell.faces.iter().copied());
        }
    }

    // Collect loop IDs per face before mutating.
    let mut face_loops: Vec<(FaceId, Vec<crate::primitives::r#loop::LoopId>)> = Vec::new();
    for &fid in &face_ids {
        if let Some(face) = model.faces.get(fid) {
            let mut loops = vec![face.outer_loop];
            loops.extend(face.inner_loops.iter().copied());
            face_loops.push((fid, loops));
        }
    }

    // Flip face orientations.
    for &fid in &face_ids {
        if let Some(face) = model.faces.get_mut(fid) {
            face.orientation = face.orientation.flipped();
        }
    }

    // Flip edge orientations inside each loop. Reverse the edge ordering
    // too so that loop traversal still emits a consistent (head→tail)
    // walk under the new face normal. Loop stores edges and orientations
    // as parallel vectors — both must be reversed in lockstep, then each
    // orientation flag inverted.
    for (_fid, loops) in face_loops {
        for lid in loops {
            if let Some(loop_entity) = model.loops.get_mut(lid) {
                loop_entity.edges.reverse();
                loop_entity.orientations.reverse();
                for o in loop_entity.orientations.iter_mut() {
                    *o = !*o;
                }
            }
        }
    }

    Ok(())
}

/// Validate transform inputs
fn validate_transform_inputs(_model: &BRepModel, transform: &Matrix4) -> OperationResult<()> {
    // Check transform is valid (no shear, etc.)
    if transform.determinant().abs() < 1e-10 {
        return Err(OperationError::InvalidGeometry(
            "Transform matrix is singular".to_string(),
        ));
    }

    Ok(())
}

/// Validate transformed solid by running the full B-Rep validation suite.
fn validate_transformed_solid(model: &BRepModel, solid_id: SolidId) -> OperationResult<()> {
    if model.solids.get(solid_id).is_none() {
        return Err(OperationError::InvalidBRep("Solid not found".to_string()));
    }
    let result = crate::primitives::validation::validate_model_enhanced(
        model,
        crate::math::Tolerance::default(),
        crate::primitives::validation::ValidationLevel::Standard,
    );
    if !result.is_valid {
        let summary = result
            .errors
            .iter()
            .take(3)
            .map(|e| format!("{:?}", e))
            .collect::<Vec<_>>()
            .join("; ");
        return Err(OperationError::InvalidBRep(format!(
            "Transformed solid failed validation ({} errors): {}",
            result.errors.len(),
            summary
        )));
    }
    Ok(())
}

// Helper functions for transform operations
// Note: Matrix4 already has all the needed transformation methods

#[cfg(test)]
#[allow(clippy::expect_used)]
#[allow(clippy::panic)]
mod tests {
    use super::*;
    use crate::operations::recorder::{OperationRecorder, RecordedOperation, RecorderError};
    use crate::primitives::topology_builder::{GeometryId, TopologyBuilder};
    use std::sync::{Arc, Mutex};

    // ────────── shared fixtures ──────────

    /// Build a 2x2x2 box centered at origin. 8 vertices at (±1, ±1, ±1).
    fn unit_box() -> (BRepModel, SolidId) {
        let mut model = BRepModel::new();
        let mut builder = TopologyBuilder::new(&mut model);
        let id = builder
            .create_box_3d(2.0, 2.0, 2.0)
            .expect("create_box_3d failed");
        let solid_id = match id {
            GeometryId::Solid(s) => s,
            other => panic!("expected Solid, got {:?}", other),
        };
        (model, solid_id)
    }

    /// Recorder that captures every emitted RecordedOperation.
    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: Mutex<Vec<RecordedOperation>>,
    }

    impl OperationRecorder for CaptureRecorder {
        fn record(&self, op: RecordedOperation) -> Result<(), RecorderError> {
            self.events
                .lock()
                .expect("CaptureRecorder mutex poisoned")
                .push(op);
            Ok(())
        }
    }

    fn captured_kinds(rec: &Arc<CaptureRecorder>) -> Vec<String> {
        rec.events
            .lock()
            .expect("mutex")
            .iter()
            .map(|e| e.kind.clone())
            .collect()
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    fn approx_pos(p: [f64; 3], q: [f64; 3]) -> bool {
        approx(p[0], q[0]) && approx(p[1], q[1]) && approx(p[2], q[2])
    }

    fn collect_positions(model: &BRepModel) -> Vec<[f64; 3]> {
        model.vertices.iter().map(|(_, v)| v.position).collect()
    }

    // ────────── A. TransformOptions ──────────

    #[test]
    fn transform_options_default_updates_parameterization() {
        let opts = TransformOptions::default();
        assert!(opts.update_parameterization);
    }

    #[test]
    fn transform_options_default_common_validate_result() {
        let opts = TransformOptions::default();
        // Default CommonOptions::validate_result is the kernel-wide default;
        // we only assert the field exists and is reachable, not its value.
        let _flag: bool = opts.common.validate_result;
    }

    // ────────── B. validate_transform_inputs ──────────

    #[test]
    fn validate_inputs_rejects_singular_matrix() {
        let mut model = BRepModel::new();
        // All-zero matrix has determinant 0.
        let m = Matrix4::scale(0.0, 0.0, 0.0);
        let err = validate_transform_inputs(&model, &m).unwrap_err();
        match err {
            OperationError::InvalidGeometry(msg) => assert!(msg.contains("singular")),
            other => panic!("expected InvalidGeometry, got {:?}", other),
        }
        // Touch model so the param isn't unused.
        let _ = &mut model;
    }

    #[test]
    fn validate_inputs_accepts_identity() {
        let model = BRepModel::new();
        assert!(validate_transform_inputs(&model, &Matrix4::identity()).is_ok());
    }

    #[test]
    fn validate_inputs_accepts_pure_translation() {
        let model = BRepModel::new();
        let m = Matrix4::translation(5.0, -3.0, 2.5);
        assert!(validate_transform_inputs(&model, &m).is_ok());
    }

    #[test]
    fn validate_inputs_accepts_reflection() {
        // Reflection has determinant -1; |det| = 1 > 1e-10 → accepted.
        let model = BRepModel::new();
        let m = Matrix4::mirror(Point3::ORIGIN, Vector3::Z).expect("mirror");
        assert!(validate_transform_inputs(&model, &m).is_ok());
        assert!((m.determinant() - -1.0).abs() < 1e-12);
    }

    #[test]
    fn validate_inputs_rejects_below_tolerance() {
        // det = 1e-12 < 1e-10 → rejected.
        let model = BRepModel::new();
        let m = Matrix4::scale(1e-4, 1e-4, 1e-4);
        assert!(m.determinant().abs() < 1e-10);
        assert!(validate_transform_inputs(&model, &m).is_err());
    }

    // ────────── C. scale public API validation ──────────

    #[test]
    fn scale_rejects_zero_x() {
        let (mut model, sid) = unit_box();
        let res = scale(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::new(0.0, 1.0, 1.0),
            TransformOptions::default(),
        );
        assert!(matches!(res, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn scale_rejects_negative_y() {
        let (mut model, sid) = unit_box();
        let res = scale(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::new(1.0, -1.0, 1.0),
            TransformOptions::default(),
        );
        assert!(matches!(res, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn scale_rejects_negative_z() {
        let (mut model, sid) = unit_box();
        let res = scale(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::new(1.0, 1.0, -2.0),
            TransformOptions::default(),
        );
        assert!(matches!(res, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn scale_accepts_isotropic_positive() {
        let (mut model, sid) = unit_box();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = scale(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::new(2.0, 2.0, 2.0),
            opts,
        )
        .expect("scale");
        assert_eq!(res.transformed_ids, vec![sid]);
    }

    // ────────── D. translate end-to-end ──────────

    #[test]
    fn translate_zero_distance_leaves_vertices_unchanged() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = translate(&mut model, vec![sid], Vector3::X, 0.0, opts).expect("translate");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx_pos(*a, *b));
        }
    }

    #[test]
    fn translate_along_x_shifts_x_only() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = translate(&mut model, vec![sid], Vector3::X, 5.0, opts).expect("translate");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx(b[0] - a[0], 5.0));
            assert!(approx(b[1], a[1]));
            assert!(approx(b[2], a[2]));
        }
    }

    #[test]
    fn translate_arbitrary_direction_distance_compose() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let dir = Vector3::new(0.0, 1.0, 0.0);
        let _ = translate(&mut model, vec![sid], dir, 3.0, opts).expect("translate");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx(b[0], a[0]));
            assert!(approx(b[1] - a[1], 3.0));
            assert!(approx(b[2], a[2]));
        }
    }

    #[test]
    fn translate_returns_input_solid_id() {
        let (mut model, sid) = unit_box();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = translate(&mut model, vec![sid], Vector3::Z, 1.0, opts).expect("translate");
        assert_eq!(res.transformed_ids, vec![sid]);
    }

    // ────────── E. rotate end-to-end ──────────

    #[test]
    fn rotate_zero_angle_leaves_vertices_unchanged() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ =
            rotate(&mut model, vec![sid], Point3::ORIGIN, Vector3::Z, 0.0, opts).expect("rotate");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx_pos(*a, *b));
        }
    }

    #[test]
    fn rotate_full_turn_returns_to_origin() {
        use std::f64::consts::PI;
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = rotate(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::Z,
            2.0 * PI,
            opts,
        )
        .expect("rotate");
        let after = collect_positions(&model);
        // Allow looser tolerance for compounded float ops.
        for (a, b) in before.iter().zip(after.iter()) {
            assert!((a[0] - b[0]).abs() < 1e-6);
            assert!((a[1] - b[1]).abs() < 1e-6);
            assert!((a[2] - b[2]).abs() < 1e-6);
        }
    }

    #[test]
    fn rotate_90_degrees_about_z_swaps_x_to_y() {
        use std::f64::consts::FRAC_PI_2;
        let (mut model, sid) = unit_box();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = rotate(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::Z,
            FRAC_PI_2,
            opts,
        )
        .expect("rotate");
        // After 90° about Z, (1, 1, _) → (-1, 1, _); in particular every
        // vertex with (x = 1, y = 1) ends with x ≈ -1.
        let after = collect_positions(&model);
        // Bounding box on x should now be [-1, 1] still, but no vertex has x ≈ 1 with y ≈ 1
        // because (1,1,*) → (-1,1,*).
        let any_x_near_neg1 = after.iter().any(|p| (p[0] + 1.0).abs() < 1e-9);
        let any_y_near_1 = after.iter().any(|p| (p[1] - 1.0).abs() < 1e-9);
        assert!(any_x_near_neg1);
        assert!(any_y_near_1);
    }

    #[test]
    fn rotate_180_about_x_negates_y_and_z() {
        use std::f64::consts::PI;
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ =
            rotate(&mut model, vec![sid], Point3::ORIGIN, Vector3::X, PI, opts).expect("rotate");
        let after = collect_positions(&model);
        // (x,y,z) → (x,-y,-z) under 180° about X.
        // Multisets must match between `before` and `(x,-y,-z)` of after.
        let mapped: Vec<[f64; 3]> = before.iter().map(|p| [p[0], -p[1], -p[2]]).collect();
        for p in &after {
            let mut found = false;
            for q in &mapped {
                if (p[0] - q[0]).abs() < 1e-9
                    && (p[1] - q[1]).abs() < 1e-9
                    && (p[2] - q[2]).abs() < 1e-9
                {
                    found = true;
                    break;
                }
            }
            assert!(found, "rotated vertex {:?} not found in expected set", p);
        }
    }

    // ────────── F. scale end-to-end ──────────

    #[test]
    fn scale_isotropic_about_origin_doubles_coords() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = scale(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::new(2.0, 2.0, 2.0),
            opts,
        )
        .expect("scale");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx(b[0], 2.0 * a[0]));
            assert!(approx(b[1], 2.0 * a[1]));
            assert!(approx(b[2], 2.0 * a[2]));
        }
    }

    #[test]
    fn scale_anisotropic_per_axis() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = scale(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::new(2.0, 3.0, 4.0),
            opts,
        )
        .expect("scale");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx(b[0], 2.0 * a[0]));
            assert!(approx(b[1], 3.0 * a[1]));
            assert!(approx(b[2], 4.0 * a[2]));
        }
    }

    #[test]
    fn scale_about_non_origin_fixes_anchor_point() {
        // Scale by 2 about point (5, 0, 0). A vertex initially at x=1
        // ends at 5 + 2*(1 - 5) = -3. A vertex at x=5 stays at x=5.
        let (mut model, sid) = unit_box();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let anchor = Point3::new(5.0, 0.0, 0.0);
        let _ = scale(
            &mut model,
            vec![sid],
            anchor,
            Vector3::new(2.0, 1.0, 1.0),
            opts,
        )
        .expect("scale");
        let after = collect_positions(&model);
        // Box vertex with original x=1 should now be at x = 5 + 2*(1-5) = -3.
        // Box vertex with original x=-1 should now be at x = 5 + 2*(-1-5) = -7.
        let xs: Vec<f64> = after.iter().map(|p| p[0]).collect();
        assert!(xs.iter().any(|&x| approx(x, -3.0)));
        assert!(xs.iter().any(|&x| approx(x, -7.0)));
    }

    // ────────── G. mirror end-to-end ──────────

    #[test]
    fn mirror_about_xy_plane_negates_z() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = mirror(&mut model, vec![sid], Point3::ORIGIN, Vector3::Z, opts).expect("mirror");
        let after = collect_positions(&model);
        // Multiset {(x,y,-z)} of before should equal multiset {(x,y,z)} of after.
        let expected: Vec<[f64; 3]> = before.iter().map(|p| [p[0], p[1], -p[2]]).collect();
        for p in &after {
            assert!(expected
                .iter()
                .any(|q| approx(p[0], q[0]) && approx(p[1], q[1]) && approx(p[2], q[2])));
        }
    }

    #[test]
    fn mirror_about_yz_plane_negates_x() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = mirror(&mut model, vec![sid], Point3::ORIGIN, Vector3::X, opts).expect("mirror");
        let after = collect_positions(&model);
        let expected: Vec<[f64; 3]> = before.iter().map(|p| [-p[0], p[1], p[2]]).collect();
        for p in &after {
            assert!(expected
                .iter()
                .any(|q| approx(p[0], q[0]) && approx(p[1], q[1]) && approx(p[2], q[2])));
        }
    }

    #[test]
    fn mirror_about_xz_plane_negates_y() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = mirror(&mut model, vec![sid], Point3::ORIGIN, Vector3::Y, opts).expect("mirror");
        let after = collect_positions(&model);
        let expected: Vec<[f64; 3]> = before.iter().map(|p| [p[0], -p[1], p[2]]).collect();
        for p in &after {
            assert!(expected
                .iter()
                .any(|q| approx(p[0], q[0]) && approx(p[1], q[1]) && approx(p[2], q[2])));
        }
    }

    #[test]
    fn mirror_flips_face_orientations() {
        let (mut model, sid) = unit_box();
        // Capture face orientations before mirror.
        let solid_before = model.solids.get(sid).expect("solid").clone();
        let shell_before = model
            .shells
            .get(solid_before.outer_shell)
            .expect("shell")
            .clone();
        let before_orients: Vec<_> = shell_before
            .faces
            .iter()
            .filter_map(|fid| model.faces.get(*fid).map(|f| (*fid, f.orientation)))
            .collect();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = mirror(&mut model, vec![sid], Point3::ORIGIN, Vector3::Z, opts).expect("mirror");
        for (fid, orig) in before_orients {
            let new = model.faces.get(fid).expect("face").orientation;
            assert_eq!(new, orig.flipped());
        }
    }

    // ────────── H. transform_solid public API ──────────

    #[test]
    fn transform_solid_identity_preserves_vertex_positions() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = transform_solid(&mut model, sid, Matrix4::identity(), opts).expect("identity");
        assert_eq!(res.transformed_ids, vec![sid]);
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx_pos(*a, *b));
        }
    }

    #[test]
    fn transform_solid_returns_supplied_matrix() {
        let (mut model, sid) = unit_box();
        let m = Matrix4::translation(7.0, -2.0, 3.5);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = transform_solid(&mut model, sid, m, opts).expect("transform");
        // The result carries back the transform that was applied.
        for i in 0..16 {
            assert!(approx(res.transform.m[i], m.m[i]));
        }
    }

    #[test]
    fn transform_solid_records_transform_solid_event() {
        let (mut model, sid) = unit_box();
        let rec: Arc<CaptureRecorder> = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(rec.clone() as Arc<dyn OperationRecorder>));
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = transform_solid(&mut model, sid, Matrix4::translation(1.0, 0.0, 0.0), opts)
            .expect("transform");
        let kinds = captured_kinds(&rec);
        assert!(kinds.iter().any(|k| k == "transform_solid"));
    }

    #[test]
    fn transform_solid_rejects_singular_matrix() {
        let (mut model, sid) = unit_box();
        let m = Matrix4::scale(0.0, 0.0, 0.0);
        let res = transform_solid(&mut model, sid, m, TransformOptions::default());
        assert!(matches!(res, Err(OperationError::InvalidGeometry(_))));
    }

    #[test]
    fn transform_solid_skip_parameterization_does_not_panic() {
        let (mut model, sid) = unit_box();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = transform_solid(&mut model, sid, Matrix4::translation(1.0, 0.0, 0.0), opts);
        assert!(res.is_ok());
    }

    // ────────── I. transform_faces ──────────

    #[test]
    fn transform_faces_empty_list_succeeds() {
        let mut model = BRepModel::new();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = transform_faces(&mut model, vec![], Matrix4::identity(), opts);
        assert!(res.is_ok());
        assert_eq!(res.expect("ok").transformed_ids.len(), 0);
    }

    #[test]
    fn transform_faces_records_transform_faces_event() {
        let (mut model, sid) = unit_box();
        let face_ids: Vec<FaceId> = {
            let solid = model.solids.get(sid).expect("solid").clone();
            let shell = model.shells.get(solid.outer_shell).expect("shell").clone();
            shell.faces
        };
        let rec: Arc<CaptureRecorder> = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(rec.clone() as Arc<dyn OperationRecorder>));
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = transform_faces(
            &mut model,
            face_ids,
            Matrix4::translation(1.0, 0.0, 0.0),
            opts,
        )
        .expect("transform_faces");
        let kinds = captured_kinds(&rec);
        assert!(kinds.iter().any(|k| k == "transform_faces"));
    }

    #[test]
    fn transform_faces_translates_vertex_positions() {
        let (mut model, sid) = unit_box();
        let face_ids: Vec<FaceId> = {
            let solid = model.solids.get(sid).expect("solid").clone();
            let shell = model.shells.get(solid.outer_shell).expect("shell").clone();
            shell.faces
        };
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = transform_faces(
            &mut model,
            face_ids,
            Matrix4::translation(0.0, 0.0, 5.0),
            opts,
        )
        .expect("transform_faces");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx(b[2] - a[2], 5.0));
        }
    }

    #[test]
    fn transform_faces_rejects_singular_matrix() {
        let mut model = BRepModel::new();
        let opts = TransformOptions::default();
        let res = transform_faces(&mut model, vec![], Matrix4::scale(0.0, 0.0, 0.0), opts);
        assert!(matches!(res, Err(OperationError::InvalidGeometry(_))));
    }

    // ────────── J. transform_edges ──────────

    #[test]
    fn transform_edges_empty_list_succeeds() {
        let mut model = BRepModel::new();
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let res = transform_edges(&mut model, vec![], Matrix4::identity(), opts);
        assert!(res.is_ok());
        assert_eq!(res.expect("ok").transformed_ids.len(), 0);
    }

    #[test]
    fn transform_edges_records_transform_edges_event() {
        let (mut model, _sid) = unit_box();
        let edge_ids: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
        assert!(!edge_ids.is_empty());
        let rec: Arc<CaptureRecorder> = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(rec.clone() as Arc<dyn OperationRecorder>));
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = transform_edges(
            &mut model,
            edge_ids,
            Matrix4::translation(1.0, 0.0, 0.0),
            opts,
        )
        .expect("transform_edges");
        let kinds = captured_kinds(&rec);
        assert!(kinds.iter().any(|k| k == "transform_edges"));
    }

    #[test]
    fn transform_edges_dedups_shared_vertices() {
        // The 12 box edges share 8 vertices; if dedup failed we'd transform
        // each shared vertex multiple times, compounding the translation.
        let (mut model, _sid) = unit_box();
        let edge_ids: Vec<EdgeId> = model.edges.iter().map(|(id, _)| id).collect();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions {
                validate_result: false,
                ..CommonOptions::default()
            },
            update_parameterization: false,
        };
        let _ = transform_edges(
            &mut model,
            edge_ids,
            Matrix4::translation(0.0, 7.0, 0.0),
            opts,
        )
        .expect("transform_edges");
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            // Translation should apply exactly once: Δy == 7, not 14, 21, …
            assert!(approx(b[1] - a[1], 7.0));
        }
    }

    #[test]
    fn transform_edges_rejects_singular_matrix() {
        let mut model = BRepModel::new();
        let opts = TransformOptions::default();
        let res = transform_edges(&mut model, vec![], Matrix4::scale(0.0, 0.0, 0.0), opts);
        assert!(matches!(res, Err(OperationError::InvalidGeometry(_))));
    }

    // ────────── Task #102: inner-loop coverage in entity collection ──────────

    /// Attach a synthetic inner (hole-style) loop to the first face of
    /// `solid_id`. The loop's four vertices live well outside the box
    /// so they remain trivially distinguishable from the original eight
    /// corner vertices. Returns the new vertex / edge IDs.
    fn attach_synthetic_inner_loop(
        model: &mut BRepModel,
        solid_id: SolidId,
    ) -> ([VertexId; 4], [EdgeId; 4]) {
        use crate::primitives::curve::Line;
        use crate::primitives::edge::{Edge, EdgeOrientation};
        use crate::primitives::r#loop::{Loop, LoopType};

        let shell_id = model.solids.get(solid_id).expect("solid").outer_shell;
        let face_id = *model
            .shells
            .get(shell_id)
            .expect("shell")
            .faces
            .first()
            .expect("shell has faces");

        let h0 = model.vertices.add_or_find(10.0, 10.0, 10.0, 1e-9);
        let h1 = model.vertices.add_or_find(11.0, 10.0, 10.0, 1e-9);
        let h2 = model.vertices.add_or_find(11.0, 11.0, 10.0, 1e-9);
        let h3 = model.vertices.add_or_find(10.0, 11.0, 10.0, 1e-9);

        let c0 = model.curves.add(Box::new(Line::new(
            Point3::new(10.0, 10.0, 10.0),
            Point3::new(11.0, 10.0, 10.0),
        )));
        let c1 = model.curves.add(Box::new(Line::new(
            Point3::new(11.0, 10.0, 10.0),
            Point3::new(11.0, 11.0, 10.0),
        )));
        let c2 = model.curves.add(Box::new(Line::new(
            Point3::new(11.0, 11.0, 10.0),
            Point3::new(10.0, 11.0, 10.0),
        )));
        let c3 = model.curves.add(Box::new(Line::new(
            Point3::new(10.0, 11.0, 10.0),
            Point3::new(10.0, 10.0, 10.0),
        )));

        let e0 = model.edges.add(Edge::new_auto_range(
            0,
            h0,
            h1,
            c0,
            EdgeOrientation::Forward,
        ));
        let e1 = model.edges.add(Edge::new_auto_range(
            0,
            h1,
            h2,
            c1,
            EdgeOrientation::Forward,
        ));
        let e2 = model.edges.add(Edge::new_auto_range(
            0,
            h2,
            h3,
            c2,
            EdgeOrientation::Forward,
        ));
        let e3 = model.edges.add(Edge::new_auto_range(
            0,
            h3,
            h0,
            c3,
            EdgeOrientation::Forward,
        ));

        let mut inner = Loop::new(0, LoopType::Inner);
        inner.add_edge(e0, true);
        inner.add_edge(e1, true);
        inner.add_edge(e2, true);
        inner.add_edge(e3, true);
        let inner_id = model.loops.add(inner);

        model
            .faces
            .get_mut(face_id)
            .expect("face exists")
            .add_inner_loop(inner_id);

        ([h0, h1, h2, h3], [e0, e1, e2, e3])
    }

    #[test]
    fn get_solid_entities_walks_inner_loops() {
        // Regression for Task #102: get_solid_entities used to walk only
        // face.outer_loop, silently dropping every vertex and edge that
        // lived on a face's inner (hole) loop. Transforming such a solid
        // would leave hole geometry sitting at the original position
        // while the outer hull moved, tearing the model.
        let (mut model, sid) = unit_box();
        let (hole_verts, hole_edges) = attach_synthetic_inner_loop(&mut model, sid);

        let entities = get_solid_entities(&model, sid).expect("get_solid_entities");

        for v in hole_verts {
            assert!(
                entities.vertices.contains(&v),
                "inner-loop vertex {v} missing from collected solid entities"
            );
        }
        for e in hole_edges {
            assert!(
                entities.edges.contains(&e),
                "inner-loop edge {e} missing from collected solid entities"
            );
        }
    }

    #[test]
    fn get_faces_entities_walks_inner_loops() {
        // Same regression as above, exercised through the per-face
        // entity collector used by `transform_faces`.
        let (mut model, sid) = unit_box();
        let (hole_verts, hole_edges) = attach_synthetic_inner_loop(&mut model, sid);

        let face_ids: Vec<FaceId> = {
            let shell = model.solids.get(sid).expect("solid").outer_shell;
            model.shells.get(shell).expect("shell").faces.clone()
        };

        let entities = get_faces_entities(&model, &face_ids).expect("get_faces_entities");

        for v in hole_verts {
            assert!(
                entities.vertices.contains(&v),
                "inner-loop vertex {v} missing from face entity collection"
            );
        }
        for e in hole_edges {
            assert!(
                entities.edges.contains(&e),
                "inner-loop edge {e} missing from face entity collection"
            );
        }
    }

    #[test]
    fn transform_solid_translates_inner_loop_vertices() {
        // End-to-end regression: a translate applied to a solid whose
        // faces carry inner (hole) loops must move the hole vertices
        // by the same vector as the outer-hull vertices. Pre-fix this
        // assertion failed because `get_solid_entities` never returned
        // the inner-loop vertices, so `transform_vertices` skipped them.
        let (mut model, sid) = unit_box();
        let (hole_verts, _hole_edges) = attach_synthetic_inner_loop(&mut model, sid);

        // Snapshot inner-loop vertex positions before the translate.
        let before: Vec<[f64; 3]> = hole_verts
            .iter()
            .map(|&v| model.vertices.get(v).expect("vertex").position)
            .collect();

        let opts = TransformOptions {
            common: CommonOptions {
                // Skip post-validation: our synthetic inner loop is not a
                // closed cycle on the cube's surface, so model-level
                // B-Rep validation would (correctly) flag it. We're
                // testing the entity-collection logic, not surface
                // membership.
                validate_result: false,
                ..CommonOptions::default()
            },
            // Surface parameterization is irrelevant — we're checking
            // vertex motion.
            update_parameterization: false,
        };
        let _ = transform_solid(&mut model, sid, Matrix4::translation(2.0, 3.0, -5.0), opts)
            .expect("transform_solid");

        for (i, &v) in hole_verts.iter().enumerate() {
            let after = model.vertices.get(v).expect("vertex").position;
            assert!(
                approx(after[0] - before[i][0], 2.0)
                    && approx(after[1] - before[i][1], 3.0)
                    && approx(after[2] - before[i][2], -5.0),
                "inner-loop vertex {v} not translated: before={:?} after={:?}",
                before[i],
                after
            );
        }
    }
}
