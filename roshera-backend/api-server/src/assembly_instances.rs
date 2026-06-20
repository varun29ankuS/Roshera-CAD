//! Positioned-instance assemblies (#19) exposed over REST.
//!
//! This is the *reference-only* assembly surface — the scaling pillar for
//! the 100-part north star. It is deliberately distinct from
//! `assembly_mgr.rs` (the mate-centric kernel `Assembly`, which copies
//! geometry per component). Here an INSTANCE is a `(part_id, transform)`
//! reference into the active part model: the same `part_id` can appear in
//! many instances, and geometry is composited at render time, never copied.
//!
//! # Why a separate manager
//!
//! `assembly_mgr::AssemblyManager` owns kernel `Assembly` values whose
//! `Component`s each hold an `Arc<BRepModel>`. Modelling "5 instances of 3
//! parts" there would mean 5 geometric copies — the opposite of instancing.
//! The kernel `InstancedAssembly` carries no geometry at all; it stores
//! `part_id` UUIDs and transforms. The geometry stays in the active model
//! (`AppState::model`), and `render_assembly` resolves each `part_id` to its
//! solid and composites with the instance transform.
//!
//! # Concurrency
//!
//! `DashMap<Uuid, Arc<RwLock<InstancedAssembly>>>` — same pattern as
//! `AssemblyManager` / `CSketchManager`. The map gives lock-free manager
//! reads; each assembly's own lock guards its instance list.
//!
//! # Phase-2 seam (mates)
//!
//! Mates are intentionally out of scope here. They belong either on the
//! kernel `InstancedAssembly` (a `Vec<InstanceMate>` driving instance
//! transforms via the existing Gauss-Seidel solver in `assembly/mod.rs`) or
//! by unifying the two assembly models once the document store makes
//! `Component` reference-only. Nothing in this file presumes a copy, so the
//! seam stays clean.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use dashmap::DashMap;
use geometry_engine::assembly::instancing::{InstanceId, InstancedAssembly};
use geometry_engine::math::Matrix4;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of positioned-instance assemblies keyed by assembly UUID.
#[derive(Default)]
pub struct InstancedAssemblyManager {
    assemblies: DashMap<Uuid, Arc<RwLock<InstancedAssembly>>>,
}

impl InstancedAssemblyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Allocate a fresh, empty assembly. Returns the kernel-assigned UUID
    /// (also the REST path id).
    pub fn create(&self, name: impl Into<String>) -> Uuid {
        let assembly = InstancedAssembly::new(name);
        let id = assembly.id;
        self.assemblies.insert(id, Arc::new(RwLock::new(assembly)));
        id
    }

    /// Cloned handle to the assembly lock. `None` for unknown ids.
    pub fn get(&self, id: &Uuid) -> Option<Arc<RwLock<InstancedAssembly>>> {
        self.assemblies.get(id).map(|e| Arc::clone(e.value()))
    }

    /// Remove an assembly. `None` when the id is unknown.
    pub fn delete(&self, id: &Uuid) -> Option<Arc<RwLock<InstancedAssembly>>> {
        self.assemblies.remove(id).map(|(_, v)| v)
    }

    /// Every live assembly id, in arbitrary order.
    pub fn list(&self) -> Vec<Uuid> {
        self.assemblies.iter().map(|e| *e.key()).collect()
    }

    pub fn len(&self) -> usize {
        self.assemblies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assemblies.is_empty()
    }
}

// ── Wire DTOs ───────────────────────────────────────────────────────

fn matrix_to_array(m: Matrix4) -> [[f64; 4]; 4] {
    let mut out = [[0.0_f64; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            out[r][c] = m[(r, c)];
        }
    }
    out
}

fn array_to_matrix(a: [[f64; 4]; 4]) -> Matrix4 {
    let mut m = Matrix4::IDENTITY;
    for r in 0..4 {
        for c in 0..4 {
            m[(r, c)] = a[r][c];
        }
    }
    m
}

/// Per-instance wire summary, including the instance's resolved perception
/// (whether its referenced part is currently a sound solid in the active
/// model). `sound = None` when the part can't be resolved (was deleted, or
/// never existed) — the assembly still lists the instance so the dangling
/// reference is visible, not silently dropped.
#[derive(Debug, Clone, Serialize)]
pub struct InstanceSummary {
    pub id: Uuid,
    pub part_id: Uuid,
    pub name: Option<String>,
    pub transform: [[f64; 4]; 4],
    pub color: Option<[u8; 3]>,
    /// Kernel solid id the part currently resolves to (if any).
    pub resolved_solid: Option<u32>,
    /// Per-instance soundness from the part's validity certificate.
    /// `None` = unresolvable part reference (dangling).
    pub sound: Option<bool>,
}

/// Assembly summary + perception: instance count, distinct-part count,
/// per-instance soundness, combined world bbox.
#[derive(Debug, Clone, Serialize)]
pub struct AssemblyInstancesSummary {
    pub id: Uuid,
    pub name: String,
    pub instance_count: usize,
    /// Distinct parts referenced. `instance_count - unique_part_count` is the
    /// number of placements served by REUSE — the instancing payoff.
    pub unique_part_count: usize,
    pub instances: Vec<InstanceSummary>,
    /// Combined world bbox `[min, max]`. `None` when no instance resolves.
    pub bbox: Option<[[f64; 3]; 2]>,
    /// True iff every resolvable instance is sound AND no instance dangles.
    pub all_sound: bool,
}

// ── Request bodies ──────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct CreateAssemblyRequest {
    pub name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CreateAssemblyResponse {
    pub id: Uuid,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddInstanceRequest {
    /// The part document UUID to instantiate. May already be referenced by
    /// other instances of this assembly — that IS the instancing.
    pub part_id: Uuid,
    /// Optional 4×4 row-major world transform. Defaults to identity.
    pub transform: Option<[[f64; 4]; 4]>,
    pub name: Option<String>,
    /// Optional per-instance display colour (RGB).
    pub color: Option<[u8; 3]>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddInstanceResponse {
    pub instance_id: Uuid,
    #[serde(flatten)]
    pub summary: AssemblyInstancesSummary,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TransformInstanceRequest {
    pub transform: [[f64; 4]; 4],
}

#[derive(Debug, Clone, Deserialize)]
pub struct ViewQuery {
    pub az: Option<f64>,
    pub el: Option<f64>,
    pub mode: Option<String>,
    pub size: Option<usize>,
    pub quality: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ViewResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub az_deg: f64,
    pub el_deg: f64,
    pub instance_count: usize,
    pub unique_part_count: usize,
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
}

// ── Helpers ─────────────────────────────────────────────────────────

fn not_found(id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::SolidNotFound,
        format!("instanced assembly {} not found", id),
    )
    .with_hint("Create one via POST /api/assembly first.")
}

fn instance_not_found(id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::SolidNotFound,
        format!("instance {} not found in assembly", id),
    )
}

/// Build the perception-bearing summary under a held read lock. Resolves
/// each instance's `part_id` to a solid in the active model, certifies it
/// for soundness, and unions transformed bboxes. Needs a `&mut BRepModel`
/// (certification warms a cache); the caller passes the active model's
/// write guard.
fn build_summary(
    assembly: &InstancedAssembly,
    state: &AppState,
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
) -> AssemblyInstancesSummary {
    let mut instances = Vec::with_capacity(assembly.instance_count());
    let mut all_sound = true;
    for inst in assembly.instances() {
        let resolved_solid = state.get_local_id(&inst.part_id);
        let sound = match resolved_solid {
            Some(sid) => {
                if model.solids.get(sid).is_some() {
                    let cert = model.certify_solid(sid);
                    Some(cert.is_sound())
                } else {
                    None
                }
            }
            None => None,
        };
        if sound != Some(true) {
            all_sound = false;
        }
        instances.push(InstanceSummary {
            id: inst.id.0,
            part_id: inst.part_id,
            name: inst.name.clone(),
            transform: matrix_to_array(inst.transform),
            color: inst.color,
            resolved_solid,
            sound,
        });
    }

    // Combined bbox: resolve each part to its world bbox in the active
    // model, transform-and-union via the kernel helper.
    let bbox = assembly
        .combined_bbox(|pid| {
            let sid = state.get_local_id(&pid)?;
            model.solid_world_bbox(sid)
        })
        .map(|bb| {
            [
                [bb.min.x, bb.min.y, bb.min.z],
                [bb.max.x, bb.max.y, bb.max.z],
            ]
        });

    AssemblyInstancesSummary {
        id: assembly.id,
        name: assembly.name.clone(),
        instance_count: assembly.instance_count(),
        unique_part_count: assembly.unique_part_count(),
        instances,
        bbox,
        all_sound,
    }
}

// ── Route handlers ──────────────────────────────────────────────────

/// `POST /api/assembly` — create an empty positioned-instance assembly.
pub async fn create_assembly(
    State(state): State<AppState>,
    Json(req): Json<CreateAssemblyRequest>,
) -> Result<Json<CreateAssemblyResponse>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "name must not be empty",
        ));
    }
    let id = state.instanced_assemblies.create(req.name);
    Ok(Json(CreateAssemblyResponse { id }))
}

/// `GET /api/assembly` — list all instanced-assembly ids.
pub async fn list_assemblies(State(state): State<AppState>) -> Json<Vec<Uuid>> {
    Json(state.instanced_assemblies.list())
}

/// `GET /api/assembly/{id}` — list instances + full perception.
pub async fn get_assembly(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
) -> Result<Json<AssemblyInstancesSummary>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    let mut model = model_handle.write().await;
    Ok(Json(build_summary(&guard, &state, &mut model)))
}

/// `DELETE /api/assembly/{id}`.
pub async fn delete_assembly(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state
        .instanced_assemblies
        .delete(&id)
        .ok_or_else(|| not_found(id))?;
    Ok(Json(serde_json::json!({ "success": true, "id": id })))
}

/// `POST /api/assembly/{id}/instance` — add a positioned instance of a part.
pub async fn add_instance(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    Json(req): Json<AddInstanceRequest>,
) -> Result<Json<AddInstanceResponse>, ApiError> {
    // Validate the part reference resolves NOW — a dangling instance is
    // allowed (parts can be deleted later) but adding one that never
    // existed is almost always a caller bug, so we reject it loudly.
    if state.get_local_id(&req.part_id).is_none() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            format!("part {} not found in the active model", req.part_id),
        )
        .with_hint("Create the part first; instances reference an existing part by id."));
    }
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| not_found(id))?;
    let transform = req
        .transform
        .map(array_to_matrix)
        .unwrap_or(Matrix4::IDENTITY);
    let (instance_id, summary) = {
        let mut guard = handle.write().await;
        let iid = guard.add_instance(req.part_id, transform, req.name.clone());
        if let Some(c) = req.color {
            guard.set_instance_color(iid, Some(c));
        }
        let mut model = model_handle.write().await;
        let summary = build_summary(&guard, &state, &mut model);
        (iid, summary)
    };
    Ok(Json(AddInstanceResponse {
        instance_id: instance_id.0,
        summary,
    }))
}

/// `PATCH /api/assembly/{id}/instance/{iid}` — re-pose an instance.
pub async fn transform_instance(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path((id, iid)): Path<(Uuid, Uuid)>,
    Json(req): Json<TransformInstanceRequest>,
) -> Result<Json<AssemblyInstancesSummary>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    if !guard.transform_instance(InstanceId(iid), array_to_matrix(req.transform)) {
        return Err(instance_not_found(iid));
    }
    let mut model = model_handle.write().await;
    Ok(Json(build_summary(&guard, &state, &mut model)))
}

/// `DELETE /api/assembly/{id}/instance/{iid}`.
pub async fn remove_instance(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path((id, iid)): Path<(Uuid, Uuid)>,
) -> Result<Json<AssemblyInstancesSummary>, ApiError> {
    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    if !guard.remove_instance(InstanceId(iid)) {
        return Err(instance_not_found(iid));
    }
    let mut model = model_handle.write().await;
    Ok(Json(build_summary(&guard, &state, &mut model)))
}

/// `GET /api/assembly/{id}/view` — composite EVERY instance into one PNG.
///
/// Each instance's `part_id` is resolved to its solid in the active model
/// and rendered at the instance transform (geometry referenced, not copied).
/// Per-instance colour: explicit instance colour wins, else the part's
/// registered scene colour, else neutral grey.
pub async fn view_assembly(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    Query(q): Query<ViewQuery>,
) -> Result<Json<ViewResponse>, ApiError> {
    use base64::Engine as _;
    use geometry_engine::math::Vector3;
    use geometry_engine::render::{render_instances_dir, CanonicalView, RenderMode, RenderOptions};
    use geometry_engine::tessellation::TessellationParams;

    let handle = state
        .instanced_assemblies
        .get(&id)
        .ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;

    let az_deg = q.az.unwrap_or(35.0);
    let el_deg = q.el.unwrap_or(20.0);
    let az = az_deg.to_radians();
    let el = el_deg.to_radians();
    let pos = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
    let dir = Vector3::new(-pos[0], -pos[1], -pos[2]);
    let up_hint = if pos[2].abs() > 0.999 {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    };
    let mode = match q.mode.as_deref().unwrap_or("shaded") {
        "shaded" => RenderMode::Shaded,
        "ids" => RenderMode::FaceIds,
        "depth" => RenderMode::Depth,
        "normals" => RenderMode::Normals,
        "diagnostic" => RenderMode::Diagnostic,
        _ => {
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                "unknown render mode",
            ))
        }
    };
    let size = q.size.unwrap_or(720).clamp(64, 2048);

    // Resolve every instance to (solid_id, transform, color). Unresolvable
    // instances are skipped — the render shows what currently exists.
    const DEFAULT: [u8; 3] = [200, 200, 200];
    let mut tuples: Vec<(u32, Matrix4, [u8; 3])> = Vec::with_capacity(guard.instance_count());
    for inst in guard.instances() {
        let Some(sid) = state.get_local_id(&inst.part_id) else {
            continue;
        };
        let color = inst
            .color
            .or_else(|| state.solid_colors.get(&sid).map(|c| *c))
            .unwrap_or(DEFAULT);
        tuples.push((sid, inst.transform, color));
    }
    if tuples.is_empty() {
        return Err(ApiError::new(
            ErrorCode::SolidNotFound,
            "assembly has no resolvable instances to render",
        ));
    }

    let model = model_handle.read().await;
    let frame = render_instances_dir(
        &model,
        &tuples,
        dir,
        up_hint,
        &RenderOptions {
            width: size,
            height: size,
            view: CanonicalView::Isometric, // ignored on the direction path
            mode,
            tessellation: match q.quality.as_deref() {
                Some("coarse") => TessellationParams::coarse(),
                Some("fine") => TessellationParams::fine(),
                _ => TessellationParams::default(),
            },
            ..Default::default()
        },
    )
    .ok_or_else(|| {
        ApiError::new(
            ErrorCode::KernelError,
            "assembly render produced no geometry",
        )
    })?;
    drop(model);

    let png = frame
        .to_png()
        .map_err(|e| ApiError::new(ErrorCode::KernelError, e))?;

    Ok(Json(ViewResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: frame.width,
        height: frame.height,
        az_deg,
        el_deg,
        instance_count: guard.instance_count(),
        unique_part_count: guard.unique_part_count(),
        open_edges: frame.open_edges,
        nonmanifold_edges: frame.nonmanifold_edges,
    }))
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_assigns_unique_ids() {
        let mgr = InstancedAssemblyManager::new();
        let a = mgr.create("A");
        let b = mgr.create("B");
        assert_ne!(a, b);
        assert_eq!(mgr.len(), 2);
        assert!(!mgr.is_empty());
    }

    #[test]
    fn delete_then_missing() {
        let mgr = InstancedAssemblyManager::new();
        let id = mgr.create("A");
        assert!(mgr.delete(&id).is_some());
        assert!(mgr.delete(&id).is_none());
        assert!(mgr.is_empty());
    }

    #[tokio::test]
    async fn instance_reuse_counts_one_part_two_placements() {
        let mgr = InstancedAssemblyManager::new();
        let id = mgr.create("rig");
        let handle = mgr.get(&id).unwrap();
        let part = Uuid::new_v4();
        let mut g = handle.write().await;
        g.add_instance(part, Matrix4::IDENTITY, None);
        let mut t = Matrix4::IDENTITY;
        t[(0, 3)] = 50.0;
        g.add_instance(part, t, None);
        assert_eq!(g.instance_count(), 2);
        assert_eq!(g.unique_part_count(), 1);
    }

    #[test]
    fn matrix_round_trip_preserves_translation() {
        let mut m = Matrix4::IDENTITY;
        m[(0, 3)] = 1.0;
        m[(1, 3)] = 2.0;
        m[(2, 3)] = 3.0;
        let a = matrix_to_array(m);
        let m2 = array_to_matrix(a);
        assert_eq!(m2[(0, 3)], 1.0);
        assert_eq!(m2[(1, 3)], 2.0);
        assert_eq!(m2[(2, 3)], 3.0);
    }

    #[test]
    fn add_instance_request_parses_minimal() {
        let raw = r#"{ "part_id": "00000000-0000-0000-0000-000000000001" }"#;
        let req: AddInstanceRequest = serde_json::from_str(raw).unwrap();
        assert!(req.transform.is_none());
        assert!(req.name.is_none());
        assert!(req.color.is_none());
    }

    #[test]
    fn add_instance_request_parses_full() {
        let raw = r#"{
            "part_id": "00000000-0000-0000-0000-000000000001",
            "transform": [[1,0,0,5],[0,1,0,0],[0,0,1,0],[0,0,0,1]],
            "name": "wheel",
            "color": [10, 20, 30]
        }"#;
        let req: AddInstanceRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.transform.unwrap()[0][3], 5.0);
        assert_eq!(req.name.as_deref(), Some("wheel"));
        assert_eq!(req.color, Some([10, 20, 30]));
    }

    #[test]
    fn not_found_uses_solid_not_found_code() {
        let err = not_found(Uuid::nil());
        assert_eq!(err.code, ErrorCode::SolidNotFound);
    }
}
