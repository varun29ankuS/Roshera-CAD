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
        apply_transform_in_place(model, solid_id, &transform, &options)?;

        // Record the operation for timeline / event-sourcing consumers.
        model.set_solid_provenance(
            solid_id,
            crate::primitives::provenance::OperationKind::Transform,
            vec![solid_id],
        );
        model.record_operation(
            crate::operations::recorder::RecordedOperation::new("transform_solid")
                .with_parameters(serde_json::json!({
                    "solid_id": solid_id,
                    "transform": transform,
                    "update_parameterization": options.update_parameterization,
                }))
                .with_input_solids([solid_id as u64])
                .with_output_solids([solid_id as u64]),
        );

        Ok(TransformResult {
            transformed_ids: vec![solid_id],
            transform,
        })
    })
}

/// Move every entity of `solid_id` through `transform`, in place, WITHOUT
/// recording. The one body shared by [`transform_solid`] (which records
/// `"transform_solid"`) and [`mirror`] (which records `"mirror"`), so a mirror
/// is exactly one event on the timeline.
///
/// An orientation-reversing matrix (negative determinant — a reflection, or a
/// reflection composed with a rigid motion or a scale) turns the image of every
/// loop the other way round, so the kernel's two orientation conventions have
/// to be restored face by face (see [`restore_orientation_after_reflection`]).
/// That needs the transformed surfaces, so an orientation-reversing transform
/// with `update_parameterization == false` is refused rather than returned
/// with its faces left to chance.
fn apply_transform_in_place(
    model: &mut BRepModel,
    solid_id: SolidId,
    transform: &Matrix4,
    options: &TransformOptions,
) -> OperationResult<()> {
    // Validate inputs
    validate_transform_inputs(model, transform)?;
    let reverses_orientation = transform.determinant() < 0.0;
    if reverses_orientation && !options.update_parameterization {
        return Err(OperationError::InvalidGeometry(
            concat!(
                "an orientation-reversing transform (negative determinant) must ",
                "carry the surfaces with it to restore each face's orientation; ",
                "update_parameterization = false would leave them behind"
            )
            .to_string(),
        ));
    }

    let solid = solid_id;

    // Get all entities in solid
    let mut entities = get_solid_entities(model, solid)?;

    // Before anything moves: one point and surface normal per face, the
    // witnesses the post-reflection orientation decision is made from.
    let normal_samples = if reverses_orientation {
        sample_face_normals(model, &entities)?
    } else {
        Vec::new()
    };
    // ...and the geometry the reflected curves and surfaces must reproduce.
    let carried = if reverses_orientation {
        Some(sample_carried_geometry(model, &entities))
    } else {
        None
    };
    let uv_reflections = if reverses_orientation {
        measured_uv_reflections(model, &entities.faces)
    } else {
        Vec::new()
    };

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
    transform_vertices(model, &entities.vertices, transform)?;

    // Transform curves
    transform_curves(model, &entities.edges, transform)?;

    // Transform surfaces
    if options.update_parameterization {
        transform_surfaces(model, &entities.faces, transform)?;
    }

    if let Some(carried) = &carried {
        reflect_measured_uv_bounds(model, &uv_reflections);
        verify_reflection_carried_geometry(model, carried, transform)?;
        restore_orientation_after_reflection(model, &normal_samples, transform)?;
    }

    // FIX 1 — carry construction geometry with the solid. A sketch-derived
    // solid records its source sketch plane + profile in the
    // `solid_construction` sidecar; without this the solid's vertices move
    // while the construction sketch stays behind, so the certificate's
    // cross-entity consistency check (FIX 2) would then — correctly — flag
    // the pair as Inconsistent. Applying the SAME matrix keeps the sketch
    // rigidly attached. No-op when the solid has no linked construction
    // geometry (analytic primitives, revolve, nurbs_loft), so those solids
    // are untouched. Identity / persistent ids are unaffected — this only
    // moves stored world points, it does not re-key the sidecar.
    if let Some(existing) = model.solid_construction.get(&solid) {
        let moved = existing.transformed(transform);
        model.solid_construction.insert(solid, moved);
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

    Ok(())
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

    // Dispatch based on entity type (the caller may pass an empty list —
    // indexing [0] would panic).
    let &first = entity_ids.first().ok_or_else(|| {
        OperationError::InvalidGeometry("translate requires an entity id".to_string())
    })?;
    transform_solid(model, first, transform, options)
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

    // Dispatch based on entity type (guard empty list — [0] would panic).
    let &first = entity_ids.first().ok_or_else(|| {
        OperationError::InvalidGeometry("rotate requires an entity id".to_string())
    })?;
    transform_solid(model, first, transform, options)
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

    // Dispatch based on entity type (guard empty list — [0] would panic).
    let &first = entity_ids.first().ok_or_else(|| {
        OperationError::InvalidGeometry("scale requires an entity id".to_string())
    })?;
    transform_solid(model, first, transform, options)
}

/// Mirror a solid through the plane at `plane_origin` with normal
/// `plane_normal`.
///
/// ONE recorded operation, of kind `"mirror"` (`solid_id`, `plane_origin`,
/// `plane_normal`, `update_parameterization`), recorded after the reflection
/// AND the per-face orientation restore, so the certificate that rides on the
/// event is of the solid this call returns. The reflection itself goes through
/// the non-recording body `transform_solid` shares, so no intermediate
/// `"transform_solid"` event reaches the timeline; replay re-runs this function
/// from the recorded plane.
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
    // Guard the empty list before entering the rollback scope ([0] would panic).
    let first = *entity_ids.first().ok_or_else(|| {
        OperationError::InvalidGeometry("mirror requires an entity id".to_string())
    })?;
    lifecycle::with_rollback(model, move |model| {
        let transform = Matrix4::mirror(plane_origin, plane_normal)?;
        apply_transform_in_place(model, first, &transform, &options)?;

        model.set_solid_provenance(
            first,
            crate::primitives::provenance::OperationKind::Transform,
            vec![first],
        );
        model.record_operation(
            crate::operations::recorder::RecordedOperation::new("mirror")
                .with_parameters(serde_json::json!({
                    "solid_id": first,
                    "plane_origin": [plane_origin.x, plane_origin.y, plane_origin.z],
                    "plane_normal": [plane_normal.x, plane_normal.y, plane_normal.z],
                    "update_parameterization": options.update_parameterization,
                }))
                .with_input_solids([first as u64])
                .with_output_solids([first as u64]),
        );

        Ok(TransformResult {
            transformed_ids: vec![first],
            transform,
        })
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

    // An orientation-reversing transform may return a curve that traces the
    // image REVERSED in its parameter (an `Arc`/`Circle` keeps `L·normal`
    // and mirrors its range; see `Arc::transformed_arc`). Such curves are
    // detected by measurement and their edges re-pointed below.
    let reflects = transform.determinant() < 0.0;
    let mut reversed: HashMap<crate::primitives::curve::CurveId, f64> = HashMap::new();

    for curve_id in curve_ids {
        // Swap the curve in-place for its transformed image. Since `Curve::transform`
        // returns a fresh `Box<dyn Curve>`, we can replace the slot directly without
        // invalidating edge references (edges keep pointing to the same CurveId).
        if let Some(slot) = model.curves.get_mut(curve_id) {
            let transformed = slot.transform(transform);
            if reflects {
                if let Some(span) = reversed_parameter_span(&**slot, &*transformed, transform) {
                    reversed.insert(curve_id, span);
                }
            }
            *slot = transformed;
        }
    }

    // An edge on a parameter-reversed curve is REVERSED with it, the way a
    // freshly built part carries it: its curve range mirrors to [s − b, s − a],
    // its start and end vertices swap, and its orientation relative to the
    // curve stays as it was — so the edge still runs along its curve's
    // parameter (the tessellator and every sampler read the curve-forward
    // samples as start → end), and `Edge::evaluate(t)` is the image of the
    // original at `1 − t`. Every loop use of such an edge inverts its sense,
    // so each loop still walks the same image path.
    if !reversed.is_empty() {
        let mut reversed_edges: HashSet<EdgeId> = HashSet::new();
        for &edge_id in edge_ids {
            // A caller's list may repeat an id; each edge is reversed ONCE
            // (a second swap would undo the first while its curve stays
            // reversed and its loop uses stay inverted).
            if reversed_edges.contains(&edge_id) {
                continue;
            }
            if let Some(edge) = model.edges.get_mut(edge_id) {
                if let Some(&span) = reversed.get(&edge.curve_id) {
                    let (a, b) = (edge.param_range.start, edge.param_range.end);
                    edge.param_range =
                        crate::primitives::curve::ParameterRange::new(span - b, span - a);
                    std::mem::swap(&mut edge.start_vertex, &mut edge.end_vertex);
                    reversed_edges.insert(edge_id);
                }
            }
        }
        let loop_ids: Vec<crate::primitives::r#loop::LoopId> = model
            .loops
            .iter()
            .filter(|(_, lp)| lp.edges.iter().any(|e| reversed_edges.contains(e)))
            .map(|(id, _)| id)
            .collect();
        for lid in loop_ids {
            if let Some(lp) = model.loops.get_mut(lid) {
                for i in 0..lp.edges.len() {
                    if reversed_edges.contains(&lp.edges[i]) {
                        if let Some(o) = lp.orientations.get_mut(i) {
                            *o = !*o;
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

/// If `new` (the image of `old` under the orientation-reversing `transform`)
/// traces that image REVERSED in its parameter — `new(s − t) = L·old(t)` —
/// return the mirror constant `s`; `None` when it traces it at the same
/// parameter (or neither, which the reflection guard then refuses).
fn reversed_parameter_span(
    old: &dyn crate::primitives::curve::Curve,
    new: &dyn crate::primitives::curve::Curve,
    transform: &Matrix4,
) -> Option<f64> {
    let r_old = old.parameter_range();
    let r_new = new.parameter_range();
    let span = 0.5 * (r_old.start + r_old.end + r_new.start + r_new.end);
    let mut same = true;
    let mut rev = true;
    for f in CARRIED_CURVE_FRACTIONS {
        let t = r_old.start + f * (r_old.end - r_old.start);
        let Ok(p) = old.point_at(t) else {
            return None;
        };
        let image = transform.transform_point(&p);
        let tol = carried_tolerance(&image);
        same &= new.point_at(t).is_ok_and(|q| q.distance(&image) <= tol);
        rev &= new
            .point_at(span - t)
            .is_ok_and(|q| q.distance(&image) <= tol);
    }
    (rev && !same).then_some(span)
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

    // Walk EVERY shell of the solid — the outer boundary, its voids
    // (`inner_shells`) and its disjoint peer bodies (`peer_shells`) — via the
    // canonical `Solid::all_shells` accessor. Walking only `outer_shell` left voids
    // and peer bodies behind on every rigid motion, mirror, datum anchoring
    // and timeline replay: the outer hull moved, the rest stayed put, and the
    // solid was torn. A face listed by more than one shell is collected once,
    // so its surface is transformed exactly once.
    let mut vertices = HashSet::new();
    let mut edges = HashSet::new();
    let mut faces: Vec<FaceId> = Vec::new();
    let mut seen_faces: HashSet<FaceId> = HashSet::new();
    for shell_id in solid.all_shells() {
        let shell = model.shells.get(shell_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "Shell {shell_id} of solid {solid_id} not found"
            ))
        })?;
        for &face_id in &shell.faces {
            if seen_faces.insert(face_id) {
                faces.push(face_id);
            }
        }
    }

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

/// Pre-transform samples of every curve and face of a solid, taken so an
/// orientation-reversing transform can be checked against them afterwards.
struct CarriedGeometry {
    /// `(edge, edge parameter, point)` at [`CARRIED_CURVE_FRACTIONS`] of each
    /// edge, walked from its start vertex to its end vertex
    /// (`Edge::evaluate`).
    curve_samples: Vec<(EdgeId, f64, Point3)>,
    /// `(face, point)`: boundary samples that lay ON the face's (trimmed)
    /// surface before the transform. Samples that did not are not recorded —
    /// a pre-existing gap is not this transform's to report.
    ///
    /// The flag records whether the sample's `closest_point` u also lay
    /// inside the face's stored u-window (surface trim and measured
    /// `uv_bounds`) before the transform; where it did, it must after.
    face_samples: Vec<(FaceId, Point3, bool)>,
}

/// Where along each edge's parameter range the reflected curve is checked.
const CARRIED_CURVE_FRACTIONS: [f64; 5] = [0.0, 0.23, 0.5, 0.71, 1.0];

/// Distance within which a sample counts as reproduced / on its surface.
fn carried_tolerance(p: &Point3) -> f64 {
    1e-6 * (1.0 + p.to_vec().magnitude())
}

/// Record [`CarriedGeometry`] for `entities`.
fn sample_carried_geometry(model: &BRepModel, entities: &SolidEntities) -> CarriedGeometry {
    let tolerance = crate::math::Tolerance::default();
    let mut curve_samples = Vec::new();
    for &edge_id in &entities.edges {
        let Some(edge) = model.edges.get(edge_id) else {
            continue;
        };
        // Asymmetric fractions: a curve traced with its parameter negated
        // agrees with the image at the ends and the middle of a full period
        // (θ = 0, π, 2π), so symmetric samples alone would not see it.
        for f in CARRIED_CURVE_FRACTIONS {
            if let Ok(p) = edge.evaluate(f, &model.curves) {
                curve_samples.push((edge_id, f, p));
            }
        }
    }

    let mut face_samples = Vec::new();
    for &face_id in &entities.faces {
        let Some(face) = model.faces.get(face_id) else {
            continue;
        };
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        let mut loops = vec![face.outer_loop];
        loops.extend(face.inner_loops.iter().copied());
        for lid in loops {
            let Some(lp) = model.loops.get(lid) else {
                continue;
            };
            for &edge_id in &lp.edges {
                let Some(edge) = model.edges.get(edge_id) else {
                    continue;
                };
                let Some(curve) = model.curves.get(edge.curve_id) else {
                    continue;
                };
                let mid = 0.5 * (edge.param_range.start + edge.param_range.end);
                let Ok(p) = curve.point_at(mid) else {
                    continue;
                };
                let Ok((u, v)) = surface.closest_point(&p, tolerance) else {
                    continue;
                };
                let on_surface = surface
                    .point_at(u, v)
                    .is_ok_and(|q| q.distance(&p) <= carried_tolerance(&p));
                if on_surface {
                    face_samples.push((face_id, p, u_in_face_window(surface, face, u)));
                }
            }
        }
    }
    CarriedGeometry {
        curve_samples,
        face_samples,
    }
}

/// Angular slack when testing a closest-point u against a stored window.
const U_WINDOW_SLACK: f64 = 1e-6;

/// Does `u` (as `closest_point` reports it) lie inside the face's stored
/// u-window: the surface's own u-trim (`parameter_bounds`) and, when measured,
/// the face's `uv_bounds`? Consumers such as the line–patch limits in
/// `operations::intersect` and `Face::contains_uv_point` compare exactly
/// these two numbers.
fn u_in_face_window(
    surface: &dyn crate::primitives::surface::Surface,
    face: &crate::primitives::face::Face,
    u: f64,
) -> bool {
    // A window spanning a full period contains every angle, whatever its
    // numeric offset; only a PARTIAL window is compared number for number,
    // which is what the consumers do.
    let within = |u: f64, lo: f64, hi: f64| {
        !lo.is_finite()
            || !hi.is_finite()
            || hi - lo >= std::f64::consts::TAU - U_WINDOW_SLACK
            || (u >= lo - U_WINDOW_SLACK && u <= hi + U_WINDOW_SLACK)
    };
    let ((a, b), _) = surface.parameter_bounds();
    let in_bounds = face
        .measured_uv_bounds()
        .is_none_or(|[u0, u1, _, _]| within(u, u0, u1));
    within(u, a, b) && in_bounds
}

/// After an orientation-reversing transform, prove the curves and surfaces
/// carried their parameterisation with it before anything is re-oriented.
///
/// The per-face restore ([`restore_orientation_after_reflection`]) reverses
/// loops or flips face flags on the premise that every transformed curve
/// traces the image of the original at the same parameter,
/// `p'(t) = L·p(t)`, and every transformed surface still contains the image
/// of its trimmed patch. A curve or surface kind whose `transform` does not
/// honour that — measured here, not assumed — would leave edges that no
/// longer end at their vertices, or faces whose trim no longer covers their
/// boundary. That is refused, naming the curve or face, rather than returned.
fn verify_reflection_carried_geometry(
    model: &BRepModel,
    carried: &CarriedGeometry,
    transform: &Matrix4,
) -> OperationResult<()> {
    let tolerance = crate::math::Tolerance::default();
    // Group the samples per edge: an edge traces the image either at the same
    // edge parameter or — when its curve came back parameter-reversed and
    // the edge was reversed with it (`transform_curves`) — at `1 − t`.
    // Either is the image; a mixture, or neither, is not.
    let mut per_edge: HashMap<EdgeId, Vec<(f64, Point3)>> = HashMap::new();
    for &(edge_id, t, before) in &carried.curve_samples {
        per_edge.entry(edge_id).or_default().push((t, before));
    }
    let mut edge_ids: Vec<EdgeId> = per_edge.keys().copied().collect();
    edge_ids.sort_unstable();
    for edge_id in edge_ids {
        let samples = per_edge.get(&edge_id).map(Vec::as_slice).unwrap_or(&[]);
        let edge = model
            .edges
            .get(edge_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("edge {edge_id} not found")))?;
        let curve = model.curves.get(edge.curve_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "curve {} of edge {edge_id} not found",
                edge.curve_id
            ))
        })?;
        let worst = |reverse: bool| -> (f64, f64) {
            let mut worst = (0.0_f64, 0.0_f64);
            for &(t, before) in samples {
                let expected = transform.transform_point(&before);
                let at = if reverse { 1.0 - t } else { t };
                let miss = edge
                    .evaluate(at, &model.curves)
                    .map(|p| p.distance(&expected) / carried_tolerance(&expected))
                    .unwrap_or(f64::INFINITY);
                if !(miss <= worst.1) {
                    worst = (t, miss);
                }
            }
            worst
        };
        let (t_fwd, fwd) = worst(false);
        let (_, rev) = worst(true);
        if !(fwd <= 1.0 || rev <= 1.0) {
            return Err(OperationError::NumericalError(format!(
                concat!(
                    "edge {} ({}): the orientation-reversing transform does not ",
                    "carry this edge — at edge parameter {} it lands {:.3e} ",
                    "tolerances from the image of the original point"
                ),
                edge_id,
                curve.type_name(),
                t_fwd,
                fwd.min(rev)
            )));
        }
    }
    for &(face_id, before, in_window_before) in &carried.face_samples {
        let expected = transform.transform_point(&before);
        let surface_id = model
            .faces
            .get(face_id)
            .map(|f| f.surface_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("face {face_id} not found")))?;
        let surface = model.surfaces.get(surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "surface {surface_id} of face {face_id} not found"
            ))
        })?;
        let uv = surface.closest_point(&expected, tolerance);
        let landed = uv
            .as_ref()
            .ok()
            .and_then(|&(u, v)| surface.point_at(u, v).ok());
        let miss = landed
            .map(|q| q.distance(&expected))
            .unwrap_or(f64::INFINITY);
        if in_window_before {
            let face = model.faces.get(face_id).ok_or_else(|| {
                OperationError::InvalidGeometry(format!("face {face_id} not found"))
            })?;
            if let Ok((u, _)) = uv {
                if !u_in_face_window(surface, face, u) {
                    return Err(OperationError::NumericalError(format!(
                        concat!(
                            "face {} ({}): after the orientation-reversing transform ",
                            "the surface parameter of a boundary point, u = {:.6}, ",
                            "falls outside the face's stored u-window; consumers ",
                            "comparing closest-point parameters with the window ",
                            "would reject the face"
                        ),
                        face_id,
                        surface.type_name(),
                        u
                    )));
                }
            }
        }
        if !(miss <= carried_tolerance(&expected)) {
            return Err(OperationError::NumericalError(format!(
                concat!(
                    "face {} ({}): after the orientation-reversing transform its ",
                    "surface no longer covers the image of its own boundary (a ",
                    "boundary point lands {:.3e} away)"
                ),
                face_id,
                surface.type_name(),
                miss
            )));
        }
    }
    Ok(())
}

/// The measured parametric domains a reflection must carry: for each face
/// on a Cylinder, Cone, Sphere or Torus whose `uv_bounds` were measured, the
/// face and the span `c = a + b` of its surface's u-window `[a, b]`, read
/// BEFORE the transform.
///
/// Those four surfaces keep their stored u-window through a reflection by
/// rotating their reference direction (`primitives::surface::
/// reflected_ref_dir`), which makes the reflected surface trace
/// `S''(u) = L·S(c − u)`. A face's u-range `[u0, u1]` inside that window
/// therefore becomes `[c − u1, c − u0]`: unchanged when the face covers the
/// whole window (the common case), and still inside
/// the window — the range the surface's `closest_point` reports — otherwise.
/// Every other surface kind carries its parameters unchanged.
fn measured_uv_reflections(model: &BRepModel, faces: &[FaceId]) -> Vec<(FaceId, f64)> {
    use crate::primitives::surface::SurfaceType;
    let mut out = Vec::new();
    for &face_id in faces {
        let Some(face) = model.faces.get(face_id) else {
            continue;
        };
        // A full-period u-range maps onto itself under the reflection
        // (u ↦ c − u is a bijection of the circle); it is left exactly as
        // measured.
        match face.measured_uv_bounds() {
            None => continue,
            Some([u0, u1, _, _]) if ((u1 - u0) - std::f64::consts::TAU).abs() <= 1e-9 => continue,
            Some(_) => {}
        }
        let Some(surface) = model.surfaces.get(face.surface_id) else {
            continue;
        };
        if matches!(
            surface.surface_type(),
            SurfaceType::Cylinder | SurfaceType::Cone | SurfaceType::Sphere | SurfaceType::Torus
        ) {
            let ((a, b), _) = surface.parameter_bounds();
            out.push((face_id, a + b));
        }
    }
    out
}

/// Apply [`measured_uv_reflections`] after the surfaces moved.
fn reflect_measured_uv_bounds(model: &mut BRepModel, reflections: &[(FaceId, f64)]) {
    for &(face_id, c) in reflections {
        if let Some(face) = model.faces.get_mut(face_id) {
            if let Some([u0, u1, v0, v1]) = face.measured_uv_bounds() {
                face.set_uv_bounds(c - u1, c - u0, v0, v1);
            }
        }
    }
}

/// One face's pre-transform orientation witness: a point on the face's
/// boundary, lying on its surface, and the surface's own (un-oriented) normal
/// there.
struct FaceNormalSample {
    face: FaceId,
    surface: crate::primitives::surface::SurfaceId,
    point: Point3,
    normal: Vector3,
}

/// Record one [`FaceNormalSample`] per face of `entities`, BEFORE the transform
/// moves anything.
///
/// Candidates are the parametric midpoints of the face's outer-loop edges, then
/// the loop's vertices — points that lie on the face's surface by construction.
/// The first candidate where the surface has a well-defined normal wins, which
/// steps off a sphere's pole or a cone's apex. A face with no such candidate is
/// refused: its orientation after the reflection could only be guessed.
fn sample_face_normals(
    model: &BRepModel,
    entities: &SolidEntities,
) -> OperationResult<Vec<FaceNormalSample>> {
    let tolerance = crate::math::Tolerance::default();
    let mut samples = Vec::with_capacity(entities.faces.len());
    for &face_id in &entities.faces {
        let face = model
            .faces
            .get(face_id)
            .ok_or_else(|| OperationError::InvalidGeometry(format!("face {face_id} not found")))?;
        let surface_id = face.surface_id;
        let surface = model.surfaces.get(surface_id).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "surface {surface_id} of face {face_id} not found"
            ))
        })?;
        let lp = model.loops.get(face.outer_loop).ok_or_else(|| {
            OperationError::InvalidGeometry(format!(
                "outer loop {} of face {face_id} not found",
                face.outer_loop
            ))
        })?;

        let mut candidates: Vec<Point3> = Vec::new();
        for &edge_id in &lp.edges {
            let Some(edge) = model.edges.get(edge_id) else {
                continue;
            };
            let Some(curve) = model.curves.get(edge.curve_id) else {
                continue;
            };
            let mid = 0.5 * (edge.param_range.start + edge.param_range.end);
            if let Ok(p) = curve.point_at(mid) {
                candidates.push(p);
            }
        }
        for &edge_id in &lp.edges {
            if let Some(edge) = model.edges.get(edge_id) {
                if let Some(p) = model.vertices.get_position(edge.start_vertex) {
                    candidates.push(Point3::new(p[0], p[1], p[2]));
                }
            }
        }

        let regular = |normal: &Vector3| {
            let len = normal.magnitude();
            len.is_finite() && len > 0.5
        };
        let on_boundary = candidates.into_iter().find_map(|point| {
            let (u, v) = surface.closest_point(&point, tolerance).ok()?;
            let normal = surface.normal_at(u, v).ok()?;
            regular(&normal).then_some(FaceNormalSample {
                face: face_id,
                surface: surface_id,
                point,
                normal,
            })
        });
        // A face whose boundary sits entirely on singular points of its
        // surface (a sphere bounded by its pole-to-pole seam) falls back to
        // interior parameters of the carrier surface. Whether a transformed
        // surface keeps or flips its normal is a property of the whole
        // connected surface, so any regular point of it decides the face.
        let sample = on_boundary.or_else(|| {
            let ((u0, u1), (v0, v1)) = surface.parameter_bounds();
            if ![u0, u1, v0, v1].iter().all(|x| x.is_finite()) {
                return None;
            }
            [0.5, 0.3, 0.7, 0.15, 0.85]
                .iter()
                .flat_map(|&a| [0.5, 0.3, 0.7].iter().map(move |&b| (a, b)))
                .find_map(|(a, b)| {
                    let (u, v) = (u0 + a * (u1 - u0), v0 + b * (v1 - v0));
                    let point = surface.point_at(u, v).ok()?;
                    let normal = surface.normal_at(u, v).ok()?;
                    regular(&normal).then_some(FaceNormalSample {
                        face: face_id,
                        surface: surface_id,
                        point,
                        normal,
                    })
                })
        });
        let sample = sample.ok_or_else(|| {
            OperationError::NumericalError(format!(
                concat!(
                    "face {} has no boundary point with a well-defined surface ",
                    "normal; its orientation after an orientation-reversing ",
                    "transform cannot be decided"
                ),
                face_id
            ))
        })?;
        samples.push(sample);
    }
    Ok(samples)
}

/// Restore the kernel's orientation conventions on every face after an
/// orientation-reversing transform (negative determinant).
///
/// The kernel carries two conventions (measured in
/// `tests/coedge_orientation_invariant.rs`): a loop's STORED walk runs
/// counter-clockwise about its surface's own normal, and `FaceOrientation`
/// maps that surface normal to the outward normal. A reflection maps the
/// outward normal of the original to the outward normal of the image (a normal
/// transforms by the inverse transpose), but it turns every loop's image the
/// other way round. Which of the two flags must change depends on how the
/// transformed SURFACE carries its normal, and that differs by surface kind:
///
/// * a surface whose normal is stored and transformed as a vector (plane,
///   cylinder, cone, sphere, torus) keeps pointing outward — its loops are
///   reversed and `FaceOrientation` is kept;
/// * a surface whose normal is the cross product of its parametric derivatives
///   (NURBS) flips with the handedness — its `FaceOrientation` is flipped and
///   its loops, already counter-clockwise about the flipped normal, are kept.
///
/// Flipping everything, as this function's predecessor did, turned every
/// analytic face inward (a mirrored box, cylinder or sphere came back
/// inside-out); flipping nothing left the NURBS faces inward. The decision is
/// therefore made per face and geometrically: the transformed surface's normal
/// at the image of the face's sample point is compared with the inverse-
/// transpose image of the sampled normal. A comparison that is neither clearly
/// aligned nor clearly opposed is refused, never guessed.
fn restore_orientation_after_reflection(
    model: &mut BRepModel,
    samples: &[FaceNormalSample],
    transform: &Matrix4,
) -> OperationResult<()> {
    let tolerance = crate::math::Tolerance::default();
    for sample in samples {
        let expected = transform.transform_normal(&sample.normal).map_err(|e| {
            OperationError::NumericalError(format!(
                "face {}: the sampled normal does not transform: {e:?}",
                sample.face
            ))
        })?;
        let image = transform.transform_point(&sample.point);
        let carried = {
            let surface = model.surfaces.get(sample.surface).ok_or_else(|| {
                OperationError::InvalidGeometry(format!(
                    "surface {} of face {} not found",
                    sample.surface, sample.face
                ))
            })?;
            let (u, v) = surface.closest_point(&image, tolerance).map_err(|e| {
                OperationError::NumericalError(format!(
                    concat!(
                        "face {}: the transformed surface does not locate the ",
                        "image of its sample point: {:?}"
                    ),
                    sample.face, e
                ))
            })?;
            surface.normal_at(u, v).map_err(|e| {
                OperationError::NumericalError(format!(
                    concat!(
                        "face {}: the transformed surface has no normal at the ",
                        "image of its sample point: {:?}"
                    ),
                    sample.face, e
                ))
            })?
        };
        let agreement = carried.dot(&expected) / carried.magnitude().max(f64::MIN_POSITIVE);
        if !agreement.is_finite() || agreement.abs() < 0.5 {
            return Err(OperationError::NumericalError(format!(
                concat!(
                    "face {}: after the orientation-reversing transform its ",
                    "surface normal is neither aligned with nor opposed to the ",
                    "transformed original (cosine {:.3}); its orientation cannot ",
                    "be decided"
                ),
                sample.face, agreement
            )));
        }

        if agreement > 0.0 {
            // The surface carried its normal outward: turn the loops back to
            // counter-clockwise about it, keep the face flag.
            let loops = {
                let face = model.faces.get(sample.face).ok_or_else(|| {
                    OperationError::InvalidGeometry(format!("face {} not found", sample.face))
                })?;
                let mut loops = vec![face.outer_loop];
                loops.extend(face.inner_loops.iter().copied());
                loops
            };
            for lid in loops {
                let loop_entity = model.loops.get_mut(lid).ok_or_else(|| {
                    OperationError::InvalidGeometry(format!(
                        "loop {lid} of face {} not found",
                        sample.face
                    ))
                })?;
                // Edges and their use-senses are parallel vectors: reverse both
                // in lockstep, then invert each sense.
                loop_entity.edges.reverse();
                loop_entity.orientations.reverse();
                for o in loop_entity.orientations.iter_mut() {
                    *o = !*o;
                }
            }
        } else {
            // The surface normal flipped with the handedness: flip the face
            // flag back to outward; the loops already run counter-clockwise
            // about the flipped surface normal.
            let face = model.faces.get_mut(sample.face).ok_or_else(|| {
                OperationError::InvalidGeometry(format!("face {} not found", sample.face))
            })?;
            face.orientation = face.orientation.flipped();
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
    // #29 — scope verdict to the transformed solid (see validate_solid_scoped).
    let result = crate::primitives::validation::validate_solid_scoped(
        model,
        solid_id,
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
            // A mirror carries the surfaces with the vertices (it has to, to
            // restore each face's orientation).
            update_parameterization: true,
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
            // A mirror carries the surfaces with the vertices (it has to, to
            // restore each face's orientation).
            update_parameterization: true,
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
            // A mirror carries the surfaces with the vertices (it has to, to
            // restore each face's orientation).
            update_parameterization: true,
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

    /// A box's faces are planes, whose normal is stored and transformed as a
    /// vector: the reflected plane already points out of the reflected box.
    /// The mirror must therefore KEEP every face flag and reverse every loop
    /// (the image of a counter-clockwise walk runs clockwise). The previous
    /// behaviour — flip every face flag too — is what turned a mirrored box
    /// inside-out; this test pinned that defect as `mirror_flips_face_orientations`.
    #[test]
    fn mirror_keeps_plane_face_flags_and_reverses_their_loops() {
        let (mut model, sid) = unit_box();
        let solid_before = model.solids.get(sid).expect("solid").clone();
        let faces: Vec<FaceId> = model
            .shells
            .get(solid_before.outer_shell)
            .expect("shell")
            .faces
            .clone();
        let before: Vec<_> = faces
            .iter()
            .map(|&fid| {
                let f = model.faces.get(fid).expect("face");
                let lp = model.loops.get(f.outer_loop).expect("loop");
                (
                    fid,
                    f.orientation,
                    lp.edges.clone(),
                    lp.orientations.clone(),
                )
            })
            .collect();
        let _ = mirror(
            &mut model,
            vec![sid],
            Point3::ORIGIN,
            Vector3::Z,
            TransformOptions::default(),
        )
        .expect("mirror");
        for (fid, orient, edges, senses) in before {
            let f = model.faces.get(fid).expect("face");
            assert_eq!(f.orientation, orient, "face {fid}: a plane keeps its flag");
            let lp = model.loops.get(f.outer_loop).expect("loop");
            let mut rev_edges = edges.clone();
            rev_edges.reverse();
            let rev_senses: Vec<bool> = senses.iter().rev().map(|s| !s).collect();
            assert_eq!(lp.edges, rev_edges, "face {fid}: loop edges reversed");
            assert_eq!(
                lp.orientations, rev_senses,
                "face {fid}: loop senses inverted"
            );
        }
    }

    #[test]
    fn orientation_reversing_transform_without_surfaces_is_refused() {
        let (mut model, sid) = unit_box();
        let before = collect_positions(&model);
        let opts = TransformOptions {
            common: CommonOptions::default(),
            update_parameterization: false,
        };
        let res = mirror(&mut model, vec![sid], Point3::ORIGIN, Vector3::Z, opts);
        match res {
            Err(OperationError::InvalidGeometry(msg)) => {
                assert!(msg.contains("update_parameterization"), "{msg}");
                assert!(!msg.contains("  "), "no whitespace runs: {msg:?}");
            }
            other => panic!("expected a typed refusal, got {other:?}"),
        }
        let after = collect_positions(&model);
        for (a, b) in before.iter().zip(after.iter()) {
            assert!(approx_pos(*a, *b), "a refused mirror moves nothing");
        }
    }

    #[test]
    fn mirror_records_exactly_one_mirror_event_after_the_orientation_fix() {
        let (mut model, sid) = unit_box();
        let rec: Arc<CaptureRecorder> = Arc::new(CaptureRecorder::default());
        model.attach_recorder(Some(rec.clone() as Arc<dyn OperationRecorder>));
        let _ = mirror(
            &mut model,
            vec![sid],
            Point3::new(3.0, 0.0, 0.0),
            Vector3::X,
            TransformOptions::default(),
        )
        .expect("mirror");
        let kinds = captured_kinds(&rec);
        assert_eq!(
            kinds,
            vec!["mirror".to_string()],
            "one mirror must record exactly one event, of kind \"mirror\""
        );
        let events = rec.events.lock().expect("mutex");
        let params = &events[0].parameters;
        assert_eq!(params["solid_id"], serde_json::json!(sid));
        assert_eq!(params["plane_origin"], serde_json::json!([3.0, 0.0, 0.0]));
        assert_eq!(params["plane_normal"], serde_json::json!([1.0, 0.0, 0.0]));
        assert_eq!(params["update_parameterization"], serde_json::json!(true));
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
    fn get_solid_entities_collects_a_face_shared_by_two_shells_once() {
        // Task 65: the walk now covers every shell. A face listed by two
        // shells of the same solid must be collected once, or its surface
        // would be transformed twice (a translate applied twice).
        use crate::primitives::shell::{Shell, ShellType};

        let (mut model, sid) = unit_box();
        let outer = model.solids.get(sid).expect("solid").outer_shell;
        let outer_faces = model.shells.get(outer).expect("shell").faces.clone();
        let shared = *outer_faces.first().expect("box has faces");

        let mut peer = Shell::new(0, ShellType::Closed);
        peer.add_face(shared);
        let peer_id = model.shells.add(peer);
        model
            .solids
            .get_mut(sid)
            .expect("solid")
            .add_peer_shell(peer_id);

        let entities = get_solid_entities(&model, sid).expect("get_solid_entities");
        assert_eq!(
            entities.faces.len(),
            outer_faces.len(),
            "a face shared by two shells must be collected exactly once: {:?}",
            entities.faces
        );
        assert_eq!(
            entities.faces.iter().filter(|&&f| f == shared).count(),
            1,
            "shared face {shared} collected more than once"
        );
    }

    #[test]
    fn get_solid_entities_refuses_a_dangling_shell_reference() {
        // A solid naming a shell the store does not hold is corrupt; the
        // walk must refuse rather than transform part of the solid.
        let (mut model, sid) = unit_box();
        model
            .solids
            .get_mut(sid)
            .expect("solid")
            .add_peer_shell(999_999);
        let err = get_solid_entities(&model, sid)
            .err()
            .expect("a dangling shell id must be refused");
        match err {
            OperationError::InvalidGeometry(msg) => {
                assert!(msg.contains("999999"), "message names the shell: {msg}");
                assert!(!msg.contains("  "), "no double spaces: {msg}");
            }
            other => panic!("expected InvalidGeometry, got {other:?}"),
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
