//! Kernel proprioception — `GET /api/kernel/state`.
//!
//! Returns a structured JSON snapshot of the live `BRepModel`: counts and
//! per-entity summaries for solids, shells, faces, edges, vertices, surfaces,
//! and curves. Read-only; safe to call concurrently with operations.
//!
//! # Why this exists
//!
//! The kernel emits geometry but does not, on its own, *report* what it
//! contains. An LLM (or human) driving the kernel via REST has no way to
//! ask "what's in the model right now?" without reading source. This
//! endpoint closes that loop — it is the kernel's introspection sense,
//! parallel to `/api/viewport/snapshot` which is the rendered-pixels sense.
//!
//! The response intentionally summarises rather than dumping every field:
//!
//! * **solids**: id, outer_shell, inner_shells, face_count
//! * **faces**: id, surface_id, surface_type, outer_loop, inner_loop_count, orientation
//! * **edges**: id, curve_id, curve_type, start_vertex, end_vertex
//! * **surfaces**: id, type_name (Plane / Cylinder / Sphere / Cone / Torus / NURBS / …)
//! * **counts**: top-level totals so callers can sanity-check before iterating
//!
//! Heavy data (control point grids, knot vectors, tessellation caches) is
//! deliberately omitted. Callers that need geometry should request export
//! (STL / OBJ / STEP) via the existing `/api/export` route.

use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use geometry_engine::primitives::topology_builder::BRepModel;
use serde::Serialize;
use serde_json::{json, Value};
use uuid::Uuid;

/// Top-level kernel state response.
#[derive(Debug, Serialize)]
pub struct KernelState {
    /// High-level counts. Fast pre-flight check before iterating entity arrays.
    pub counts: Counts,
    /// One entry per solid in the model.
    pub solids: Vec<SolidSummary>,
    /// One entry per face. Trimmed to `face_limit` if the model is large.
    pub faces: Vec<FaceSummary>,
    /// One entry per edge (also trimmed).
    pub edges: Vec<EdgeSummary>,
    /// One entry per surface.
    pub surfaces: Vec<SurfaceSummary>,
    /// Diagnostics flags + arbitrary kernel-internal notes.
    pub diagnostics: Value,
}

#[derive(Debug, Serialize)]
pub struct Counts {
    pub solids: usize,
    pub shells: usize,
    pub faces: usize,
    pub loops: usize,
    pub edges: usize,
    pub vertices: usize,
    pub surfaces: usize,
    pub curves: usize,
    pub sketch_planes: usize,
}

#[derive(Debug, Serialize)]
pub struct SolidSummary {
    pub id: u32,
    pub outer_shell: u32,
    pub inner_shell_count: usize,
    pub face_count: usize,
}

#[derive(Debug, Serialize)]
pub struct FaceSummary {
    pub id: u32,
    pub surface_id: u32,
    pub surface_type: String,
    pub outer_loop: u32,
    pub inner_loop_count: usize,
    pub orientation: String,
}

#[derive(Debug, Serialize)]
pub struct EdgeSummary {
    pub id: u32,
    pub curve_id: u32,
    pub start_vertex: u32,
    pub end_vertex: u32,
    pub closed: bool,
}

#[derive(Debug, Serialize)]
pub struct SurfaceSummary {
    pub id: u32,
    pub type_name: String,
}

/// Maximum number of per-entity rows to include before truncating.
/// A model with millions of faces should not produce a multi-megabyte JSON
/// response — counts still report the full totals so callers can detect
/// truncation by comparing `counts.faces` to `faces.len()`.
const ENTITY_LIMIT: usize = 4096;

/// `GET /api/kernel/state` — kernel introspection handler.
pub async fn kernel_state(
    State(state): State<AppState>,
) -> Result<Json<KernelState>, (StatusCode, Json<Value>)> {
    let model = state.model.read().await;
    let snapshot = build_snapshot(&model);
    Ok(Json(snapshot))
}

/// Pure synchronous snapshot builder. Separated from the async handler
/// so it can be exercised directly from tests without spinning up the
/// HTTP layer.
pub fn build_snapshot(model: &BRepModel) -> KernelState {
    let counts = Counts {
        solids: model.solids.len(),
        shells: model.shells.len(),
        faces: model.faces.len(),
        loops: model.loops.len(),
        edges: model.edges.len(),
        vertices: model.vertices.len(),
        surfaces: model.surfaces.len(),
        curves: model.curves.len(),
        sketch_planes: model.sketch_planes.len(),
    };

    let solids: Vec<SolidSummary> = model
        .solids
        .iter()
        .take(ENTITY_LIMIT)
        .map(|(id, solid)| {
            // Face count = sum over outer_shell + inner_shells of shell.faces.len().
            // If a referenced shell is missing, count what we can find rather
            // than failing — the diagnostics block flags structural issues.
            let mut face_count = 0usize;
            if let Some(shell) = model.shells.get(solid.outer_shell) {
                face_count += shell.faces.len();
            }
            for &inner in &solid.inner_shells {
                if let Some(shell) = model.shells.get(inner) {
                    face_count += shell.faces.len();
                }
            }
            SolidSummary {
                id,
                outer_shell: solid.outer_shell,
                inner_shell_count: solid.inner_shells.len(),
                face_count,
            }
        })
        .collect();

    let faces: Vec<FaceSummary> = model
        .faces
        .iter()
        .take(ENTITY_LIMIT)
        .map(|(id, face)| {
            let surface_type = model
                .surfaces
                .get(face.surface_id)
                .map(|s| s.type_name().to_string())
                .unwrap_or_else(|| "<missing>".to_string());
            FaceSummary {
                id,
                surface_id: face.surface_id,
                surface_type,
                outer_loop: face.outer_loop,
                inner_loop_count: face.inner_loops.len(),
                orientation: format!("{:?}", face.orientation),
            }
        })
        .collect();

    let edges: Vec<EdgeSummary> = model
        .edges
        .iter()
        .take(ENTITY_LIMIT)
        .map(|(id, edge)| EdgeSummary {
            id,
            curve_id: edge.curve_id,
            start_vertex: edge.start_vertex,
            end_vertex: edge.end_vertex,
            closed: edge.start_vertex == edge.end_vertex,
        })
        .collect();

    let surfaces: Vec<SurfaceSummary> = model
        .surfaces
        .iter()
        .take(ENTITY_LIMIT)
        .map(|(id, surf)| SurfaceSummary {
            id,
            type_name: surf.type_name().to_string(),
        })
        .collect();

    // Diagnostics — surface-type histogram, dangling-reference count,
    // and a "looks healthy" boolean. Cheap to compute, useful for the
    // LLM driving operations.
    let mut surface_histogram = std::collections::BTreeMap::<String, usize>::new();
    for (_, surf) in model.surfaces.iter() {
        *surface_histogram
            .entry(surf.type_name().to_string())
            .or_insert(0) += 1;
    }

    // Count faces whose surface_id no longer resolves — a structural
    // invariant violation that should be zero on a healthy model.
    let dangling_face_surfaces = model
        .faces
        .iter()
        .filter(|(_, f)| model.surfaces.get(f.surface_id).is_none())
        .count();

    // Same check for edges → curves.
    let dangling_edge_curves = model
        .edges
        .iter()
        .filter(|(_, e)| model.curves.get(e.curve_id).is_none())
        .count();

    let diagnostics = json!({
        "surface_type_histogram": surface_histogram,
        "dangling_face_surfaces": dangling_face_surfaces,
        "dangling_edge_curves": dangling_edge_curves,
        "healthy": dangling_face_surfaces == 0 && dangling_edge_curves == 0,
        "entity_limit": ENTITY_LIMIT,
        "truncated": counts.faces > ENTITY_LIMIT
            || counts.edges > ENTITY_LIMIT
            || counts.surfaces > ENTITY_LIMIT
            || counts.solids > ENTITY_LIMIT,
    });

    KernelState {
        counts,
        solids,
        faces,
        edges,
        surfaces,
        diagnostics,
    }
}

/// Mass properties response — wraps the kernel's `SolidMassProperties`
/// in a JSON-friendly shape. We don't `#[derive(Serialize)]` directly on
/// the kernel struct because it lives in `geometry-engine` and adding a
/// serde dep there for one HTTP shape would couple layers needlessly.
#[derive(Debug, Serialize)]
pub struct PropertiesResponse {
    pub solid_id: u32,
    pub solid_uuid: Option<String>,
    /// Geometric volume from divergence-theorem integration over the
    /// outer shell, with inner shells subtracted. Units = (model units)³.
    pub volume: f64,
    /// Total surface area summed across all shells. Units = (model units)².
    pub surface_area: f64,
    /// Mass = volume × material density (default density = 1.0 → mass = volume).
    pub mass: f64,
    /// Center of mass in model coordinates.
    pub center_of_mass: [f64; 3],
    /// 3×3 inertia tensor about the center of mass (row-major).
    pub inertia_tensor: [[f64; 3]; 3],
    /// Diagonal of the inertia tensor in the principal-axes frame.
    pub principal_moments: [f64; 3],
    /// Principal-axes frame as 3 column vectors.
    pub principal_axes: [[f64; 3]; 3],
    /// Radius of gyration along each principal axis.
    pub radius_of_gyration: [f64; 3],
    /// Axis-aligned bounding box `[min, max]`.
    pub bounding_box: [[f64; 3]; 2],
}

/// `GET /api/geometry/{id}/properties` — real mass properties via the
/// kernel's divergence-theorem integration.
///
/// `id` accepts either:
/// * the API-side UUID minted by `create_geometry` / `boolean_operation`, or
/// * the local `u32` `SolidId` (as a decimal string) — useful for kernel
///   solids that exist before the UUID mapping is attached.
///
/// Takes a write lock on the model because `Solid::compute_mass_properties`
/// caches its result on the solid; the lock window is short (one O(F+E)
/// integration pass).
pub async fn solid_properties(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<PropertiesResponse>, (StatusCode, Json<Value>)> {
    // Resolve UUID → local SolidId, falling back to a direct numeric parse.
    let (solid_id, solid_uuid) = match Uuid::parse_str(&id) {
        Ok(uuid) => match state.uuid_to_local.get(&uuid) {
            Some(local) => (*local, Some(uuid.to_string())),
            None => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": "no solid registered for this uuid",
                        "uuid": uuid.to_string(),
                    })),
                ))
            }
        },
        Err(_) => match id.parse::<u32>() {
            Ok(local) => (local, None),
            Err(_) => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(json!({
                        "error": "id must be a UUID or a decimal SolidId",
                        "received": id,
                    })),
                ))
            }
        },
    };

    let mut model = state.model.write().await;

    // Disjoint-field-borrow the stores so the kernel signature is satisfied.
    // `compute_mass_properties` mutates only the Solid's cache + the shell-
    // level cache; the mutable borrows on shell/face/loop stores are for
    // their own caches, not for topology mutation.
    let model = &mut *model;

    // Pre-compute the bbox before the long mutable borrow on solids.
    let bbox = {
        match model.solids.get_mut(solid_id) {
            Some(solid) => match solid.bounding_box(
                &model.shells,
                &model.faces,
                &model.loops,
                &model.vertices,
                &model.edges,
            ) {
                Ok((min, max)) => [[min.x, min.y, min.z], [max.x, max.y, max.z]],
                Err(e) => {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "error": "failed to compute bounding box",
                            "details": e.to_string(),
                            "solid_id": solid_id,
                        })),
                    ));
                }
            },
            None => {
                return Err((
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "error": "solid id does not exist in the model",
                        "solid_id": solid_id,
                    })),
                ));
            }
        }
    };

    // Surface area first — uses immutable surfaces but mutable shells/faces/loops.
    let surface_area = {
        let solid = model
            .solids
            .get_mut(solid_id)
            .expect("solid existed two lines up; nothing has dropped it");
        match solid.surface_area(
            &mut model.shells,
            &mut model.faces,
            &mut model.loops,
            &model.vertices,
            &model.edges,
            &model.surfaces,
            geometry_engine::math::Tolerance::default(),
        ) {
            Ok(a) => a,
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "error": "failed to compute surface area",
                        "details": e.to_string(),
                        "solid_id": solid_id,
                    })),
                ));
            }
        }
    };

    // Mass properties.
    let solid = model
        .solids
        .get_mut(solid_id)
        .expect("solid still alive in the same write lock");
    let props = match solid.compute_mass_properties(
        &mut model.shells,
        &mut model.faces,
        &mut model.loops,
        &model.vertices,
        &model.edges,
        &model.curves,
        &model.surfaces,
    ) {
        Ok(p) => p.clone(),
        Err(e) => {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "error": "failed to compute mass properties",
                    "details": e.to_string(),
                    "solid_id": solid_id,
                })),
            ));
        }
    };

    Ok(Json(PropertiesResponse {
        solid_id,
        solid_uuid,
        volume: props.volume,
        surface_area,
        mass: props.mass,
        center_of_mass: [
            props.center_of_mass.x,
            props.center_of_mass.y,
            props.center_of_mass.z,
        ],
        inertia_tensor: props.inertia_tensor,
        principal_moments: [
            props.principal_moments.x,
            props.principal_moments.y,
            props.principal_moments.z,
        ],
        principal_axes: [
            [
                props.principal_axes[0].x,
                props.principal_axes[0].y,
                props.principal_axes[0].z,
            ],
            [
                props.principal_axes[1].x,
                props.principal_axes[1].y,
                props.principal_axes[1].z,
            ],
            [
                props.principal_axes[2].x,
                props.principal_axes[2].y,
                props.principal_axes[2].z,
            ],
        ],
        radius_of_gyration: [
            props.radius_of_gyration.x,
            props.radius_of_gyration.y,
            props.radius_of_gyration.z,
        ],
        bounding_box: bbox,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_model_snapshot_is_healthy() {
        let model = BRepModel::new();
        let snap = build_snapshot(&model);
        assert_eq!(snap.counts.solids, 0);
        assert_eq!(snap.counts.faces, 0);
        assert_eq!(snap.diagnostics["healthy"], json!(true));
        assert_eq!(snap.diagnostics["dangling_face_surfaces"], json!(0));
    }
}
