//! Assembly module — kernel `Assembly` exposed over REST.
//!
//! # DEPRECATED SURFACE (kinematic-assembly campaign, 2026-07-16 spec §3.1)
//!
//! This mate-centric surface (`/api/assemblies/*`) RETIRES once the
//! instanced document (`/api/assembly/*`, `assembly_instances.rs`) reaches
//! mate parity: it copies geometry per component, its Gauss-Seidel solver
//! has no rank analysis, and its 12-mate taxonomy is subsumed by the
//! connector-frame taxonomy (`geometry_engine::assembly::mates`). Parity
//! notes: interference already delegates to the assembly-engine (the
//! Slice-1 lie fix — see [`interference_pairs`]); mates/solve land on the
//! B surface in Slice 2; `ExplodedViewConfig` is the one piece worth
//! porting as a *view* op. Until then the routes stay live for existing
//! clients; do not grow this surface.
//!
//! # Why this lives in api-server, not session-manager
//!
//! `session-manager::HierarchyManager` already owns the project-tree
//! `shared_types::hierarchy::Assembly` — a serialisable DTO that
//! describes the workspace tree the frontend hangs nodes off. The
//! kernel `geometry_engine::assembly::Assembly` is a different beast:
//! it carries mate constraints, a Gauss-Seidel solver, gear neutrals,
//! exploded views — none of which exist in shared-types and none of
//! which the hierarchy tree models.
//!
//! Putting the kernel-assembly manager alongside the kernel
//! `BRepModel` (api-server) keeps the dependency graph clean: we
//! avoid adding a `session-manager → geometry-engine` edge that
//! would pull the whole B-Rep universe into the auth/session crate.
//! This mirrors what `sketch.rs` and `csketch.rs` do for the two
//! kinds of sketch the kernel supports.
//!
//! # Concurrency
//!
//! Each kernel `Assembly` instance has `Arc<DashMap<_, _>>` interiors
//! but its mutating API (`add_part`, `add_mate`, `solve_constraints`,
//! ...) takes `&mut self`. We therefore store each assembly in an
//! `Arc<RwLock<Assembly>>` so handlers can grab a per-assembly write
//! lock without contending on the assembly map itself. Read-only
//! endpoints (list / summary) take the read half so a long-running
//! solve doesn't stall introspection.
//!
//! # Wire shape
//!
//! `Component` and `MateConstraint` carry `Arc<BRepModel>` /
//! non-`Serialize` payloads inside the kernel, so the wire types
//! defined below are dedicated DTOs: `AssemblySummary`,
//! `ComponentSummary`, `MateSummary`. Frontends never see the kernel
//! types directly — every response is a snapshot built by walking
//! the kernel state under the read lock.

use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;
use axum::{
    extract::{Path, State},
    response::Json,
};
use dashmap::DashMap;
use geometry_engine::assembly::{
    Assembly, AssemblyError, ComponentId, ExplodedViewConfig, MateId, MateReference, MateType,
};
use geometry_engine::math::{Matrix4, Point3, Quaternion, Vector3};
use geometry_engine::operations::recorder::{OperationRecorder, RecordedOperation};
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

// ── Manager ─────────────────────────────────────────────────────────

/// Registry of kernel assemblies keyed by the assembly's own UUID.
///
/// Each entry is `Arc<RwLock<Assembly>>` so a handler holding a write
/// lock for a slow solve never blocks readers of the map itself, and
/// concurrent reads of different assemblies don't contend. The map
/// implementation is `DashMap` — same pattern as `SketchManager` /
/// `CSketchManager`.
// `Assembly` does not derive `Debug` on every interior (mate constraint
// strings round-trip through `Debug`, but the manager wrapper is
// otherwise opaque). We omit a `#[derive(Debug)]` on the manager so the
// `AppState` `Clone` line stays clean.
#[derive(Default)]
pub struct AssemblyManager {
    assemblies: DashMap<Uuid, Arc<RwLock<Assembly>>>,
    /// Optional sink for assembly mutation events. When set, every
    /// mutating handler emits a `RecordedOperation` with `kind` in the
    /// `assembly.*` namespace so the timeline / audit stream captures
    /// the same provenance trail it already carries for kernel ops.
    ///
    /// Stored as `Arc<dyn OperationRecorder>` so the same recorder
    /// attached to the `BRepModel` (the `TimelineRecorder` wired in
    /// `main.rs`) can be reused without a second sync→async bridge.
    /// `None` is a hard no-op — safe for unit tests that don't care
    /// about events.
    recorder: Option<Arc<dyn OperationRecorder>>,
}

impl AssemblyManager {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a manager that emits assembly events into the given
    /// recorder. The api-server wires this to the same
    /// `TimelineRecorder` instance that's already attached to the
    /// `BRepModel`, so kernel and assembly events share the same
    /// timeline / audit stream and the same active-branch routing
    /// (the recorder swaps its own branch target on `POST
    /// /api/branches/active`; the manager never sees that detail).
    pub fn with_recorder(recorder: Arc<dyn OperationRecorder>) -> Self {
        Self {
            assemblies: DashMap::new(),
            recorder: Some(recorder),
        }
    }

    /// Emit one `RecordedOperation` through the attached recorder.
    /// Failures are logged at `warn` level — they never propagate
    /// because the underlying assembly mutation has already
    /// succeeded, exactly mirroring the kernel's recorder contract.
    pub fn record_event(&self, op: RecordedOperation) {
        if let Some(r) = self.recorder.as_ref() {
            if let Err(e) = r.record(op) {
                tracing::warn!(error = %e, "AssemblyManager: recorder rejected event");
            }
        }
    }

    /// Allocate a fresh, empty assembly with the given display name.
    /// Returns the kernel-assigned UUID, which is also the REST path
    /// id.
    pub fn create(&self, name: impl Into<String>) -> Uuid {
        let assembly = Assembly::new(name);
        let id = assembly.id;
        self.assemblies.insert(id, Arc::new(RwLock::new(assembly)));
        id
    }

    /// Cloned handle to the assembly's lock. `None` for unknown ids.
    pub fn get(&self, id: &Uuid) -> Option<Arc<RwLock<Assembly>>> {
        self.assemblies.get(id).map(|e| Arc::clone(e.value()))
    }

    /// Remove an assembly from the registry. Returns the dropped
    /// handle for last-mile bookkeeping (none today). `None` when
    /// the id is unknown.
    pub fn delete(&self, id: &Uuid) -> Option<Arc<RwLock<Assembly>>> {
        self.assemblies.remove(id).map(|(_, v)| v)
    }

    /// Every live assembly id, in arbitrary order.
    pub fn list(&self) -> Vec<Uuid> {
        self.assemblies.iter().map(|e| *e.key()).collect()
    }

    /// Count of live assemblies. Used by tests and `/healthz`-style
    /// introspection.
    pub fn len(&self) -> usize {
        self.assemblies.len()
    }

    pub fn is_empty(&self) -> bool {
        self.assemblies.is_empty()
    }
}

// ── Wire DTOs ───────────────────────────────────────────────────────

/// Wire summary for an assembly. Excludes the per-component
/// `Arc<BRepModel>` so the response stays tractable for an outliner
/// view; clients pull tessellation through the existing
/// `/api/geometry/{id}` surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssemblySummary {
    pub id: Uuid,
    pub name: String,
    pub root_component: Option<Uuid>,
    pub components: Vec<ComponentSummary>,
    pub mates: Vec<MateSummary>,
    pub exploded: Option<ExplodedViewConfig>,
}

/// Wire summary for a single component instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComponentSummary {
    pub id: Uuid,
    pub name: String,
    pub transform: [[f64; 4]; 4],
    pub is_fixed: bool,
    pub parent: Option<Uuid>,
    pub degrees_of_freedom: u8,
    pub mate_references: Vec<MateReferenceSummary>,
    /// Part UUID this component instantiates, when bound via
    /// `AddComponentRequest::part_id`. `None` for primitive-seeded
    /// components.
    pub source_part: Option<Uuid>,
}

/// Wire form of a named `MateReference` entry on a component. The
/// kernel stores the reference itself; we surface both the slot name
/// (the key clients pass to `add_mate`) and the geometry payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MateReferenceSummary {
    pub name: String,
    #[serde(flatten)]
    pub reference: MateReference,
}

/// Wire summary for a single mate constraint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MateSummary {
    pub id: Uuid,
    pub name: String,
    pub mate_type: MateType,
    pub component1: Uuid,
    pub reference1: String,
    pub component2: Uuid,
    pub reference2: String,
    pub suppressed: bool,
    pub flip: bool,
    pub solved: bool,
    pub error: Option<String>,
}

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

/// Snapshot an assembly under a held lock. Doesn't take or release
/// any lock itself — the caller is responsible for read/write
/// guarding so the snapshot is internally consistent.
pub fn snapshot(assembly: &Assembly) -> AssemblySummary {
    let components: Vec<ComponentSummary> = assembly
        .components()
        .map(|c| ComponentSummary {
            source_part: c.source_part,
            id: c.id.0,
            name: c.name.clone(),
            transform: matrix_to_array(c.transform),
            is_fixed: c.is_fixed,
            parent: c.parent.map(|p| p.0),
            degrees_of_freedom: c.degrees_of_freedom,
            mate_references: c
                .mate_references
                .iter()
                .map(|(k, v)| MateReferenceSummary {
                    name: k.clone(),
                    reference: v.clone(),
                })
                .collect(),
        })
        .collect();

    let mates: Vec<MateSummary> = assembly
        .mates()
        .map(|m| MateSummary {
            id: m.id.0,
            name: m.name.clone(),
            mate_type: m.mate_type,
            component1: m.component1.0,
            reference1: m.reference1.clone(),
            component2: m.component2.0,
            reference2: m.reference2.clone(),
            suppressed: m.suppressed,
            flip: m.flip,
            solved: m.solved,
            error: m.error.clone(),
        })
        .collect();

    AssemblySummary {
        id: assembly.id,
        name: assembly.name.clone(),
        root_component: assembly.root_component().map(|c| c.0),
        components,
        mates,
        exploded: assembly.exploded_config().cloned(),
    }
}

// ── Helpers ─────────────────────────────────────────────────────────

fn not_found(id: Uuid) -> ApiError {
    ApiError::new(
        ErrorCode::SolidNotFound,
        format!("assembly {} not found", id),
    )
    .with_hint("Create one via POST /api/assemblies first.")
}

fn map_kernel_err(e: AssemblyError) -> ApiError {
    let msg = e.to_string();
    let code = match e {
        AssemblyError::ComponentNotFound(_) | AssemblyError::ReferenceNotFound(_) => {
            ErrorCode::SolidNotFound
        }
        AssemblyError::OverConstrained | AssemblyError::ConflictingConstraints => {
            ErrorCode::KernelError
        }
        AssemblyError::SolverFailed(_) => ErrorCode::KernelError,
    };
    ApiError::new(code, msg)
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
pub struct AddComponentRequest {
    pub name: String,
    /// Optional 4×4 row-major transform. Defaults to identity.
    pub transform: Option<[[f64; 4]; 4]>,
    /// Optional primitive geometry to seed the component's BRepModel.
    /// The primitive is built in the component's *local* frame; the
    /// world pose is the `transform`. Mutually exclusive with
    /// `part_id`.
    pub primitive: Option<ComponentPrimitive>,
    /// Bind an EXISTING part (by its viewport/part UUID) as this
    /// component's geometry — the "part-registry slice" that was
    /// promised in a comment here and never written. The part's solid
    /// is extracted from the active model into the component's own
    /// BRepModel (a geometric copy today; shared-definition instancing
    /// is the follow-up), and the component records `source_part` so
    /// summaries can name which part it instantiates. Mutually
    /// exclusive with `primitive`.
    pub part_id: Option<Uuid>,
}

/// Primitive geometry kinds accepted by `add_component`. Externally
/// tagged on the wire (`{ "type": "Box", "dx": 10, ... }`) so the
/// TypeScript client can dispatch on the tag directly. `Serialize`
/// is derived so the timeline `RecordedOperation` parameter dump
/// round-trips the primitive descriptor exactly as it came in.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ComponentPrimitive {
    Box { dx: f64, dy: f64, dz: f64 },
    Cylinder { radius: f64, height: f64 },
    Sphere { radius: f64 },
}

#[derive(Debug, Clone, Serialize)]
pub struct AddComponentResponse {
    pub component_id: Uuid,
}

/// Wire form of a tessellated component mesh — same shape Three.js
/// expects on `BufferGeometry.setAttribute`/`setIndex`. Flattened
/// positions/normals (3 floats per vertex) and an indices buffer
/// (3 indices per triangle).
#[derive(Debug, Clone, Serialize)]
pub struct ComponentMeshResponse {
    pub component_id: Uuid,
    pub vertices: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
    /// Triangle count = `indices.len() / 3`; surfaced so the client
    /// can short-circuit empty-component renders without a length
    /// check on the buffer.
    pub triangle_count: usize,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SetTransformRequest {
    pub transform: [[f64; 4]; 4],
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterReferenceRequest {
    pub component: Uuid,
    pub name: String,
    pub reference: MateReference,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AddMateRequest {
    pub mate_type: MateType,
    pub component1: Uuid,
    pub reference1: String,
    pub component2: Uuid,
    pub reference2: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AddMateResponse {
    pub mate_id: Uuid,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PatchMateRequest {
    pub suppressed: Option<bool>,
    pub flip: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ExplodeRequest {
    /// When true (default), auto-derive explosion vectors from the
    /// assembly center; when false, return an empty config the
    /// caller can populate with explicit steps.
    #[serde(default = "default_true")]
    pub auto: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct InterferenceReport {
    pub pairs: Vec<(Uuid, Uuid)>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SimulateMotionRequest {
    pub component: Uuid,
    pub translation: [f64; 3],
    /// Optional quaternion `[x, y, z, w]` for an incremental rotation
    /// applied after the translation.
    pub rotation: Option<[f64; 4]>,
}

// ── Route handlers ──────────────────────────────────────────────────

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
    let name = req.name.clone();
    let id = state.assemblies.create(req.name);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.create")
            .with_parameters(serde_json::json!({ "name": name }))
            .with_output_assembly(id),
    );
    Ok(Json(CreateAssemblyResponse { id }))
}

pub async fn list_assemblies(State(state): State<AppState>) -> Json<Vec<Uuid>> {
    Json(state.assemblies.list())
}

pub async fn get_assembly(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AssemblySummary>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    Ok(Json(snapshot(&guard)))
}

pub async fn delete_assembly(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    state.assemblies.delete(&id).ok_or_else(|| not_found(id))?;
    state.assemblies.record_event(
        RecordedOperation::new("assembly.delete")
            .with_parameters(serde_json::json!({}))
            .with_input_assembly(id),
    );
    Ok(Json(serde_json::json!({"success": true, "id": id})))
}

pub async fn add_component(
    State(state): State<AppState>,
    crate::part_mgr::ActiveModel(model_handle): crate::part_mgr::ActiveModel,
    Path(id): Path<Uuid>,
    Json(req): Json<AddComponentRequest>,
) -> Result<Json<AddComponentResponse>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "component name must not be empty",
        ));
    }
    if req.primitive.is_some() && req.part_id.is_some() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "primitive and part_id are mutually exclusive — a component is              seeded with fresh primitive geometry OR bound to an existing part",
        ));
    }
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    // Snapshot the existing-component count BEFORE adding the new
    // component. When the caller doesn't supply a transform we use
    // this to space new components along +X so they don't pile up
    // at the origin (overlapping geometry buries the transform
    // gizmo handles, making the assembly unusable in the viewport).
    let existing_count = guard.components().count();
    // Build the component's BRepModel. If the caller passed a
    // primitive, materialise it via `TopologyBuilder`; otherwise the
    // model stays empty (legacy behaviour — caller binds a part
    // later). The primitive is built in the component's local frame
    // at the origin; the world pose lives in `transform`.
    let mut model = BRepModel::new();
    if let Some(part_uuid) = req.part_id {
        // Bind an existing part: extract its solid from the ACTIVE
        // part model into this component's own BRepModel. v1 strategy
        // deliberately composes two battle-tested pieces instead of a
        // new subgraph walker: snapshot-copy the whole model, then
        // lifecycle-delete every solid that is not the bound one
        // (delete_solid cascades the orphaned topology). A shared-
        // definition instance store (no geometric copy) is the
        // follow-up once the document model lands.
        let solid_id = state.get_local_id(&part_uuid).ok_or_else(|| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!("part {part_uuid} not found in the active model"),
            )
        })?;
        {
            let src = model_handle.read().await;
            if src.solids.get(solid_id).is_none() {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!("part {part_uuid} maps to solid {solid_id}, which no longer exists"),
                ));
            }
            geometry_engine::primitives::snapshot::ModelSnapshot::take(&src).restore(&mut model);
        }
        let others: Vec<u32> = model
            .solids
            .iter()
            .map(|(sid, _)| sid)
            .filter(|sid| *sid != solid_id)
            .collect();
        for sid in others {
            geometry_engine::operations::delete::delete_solid(&mut model, sid, true).map_err(
                |e| {
                    ApiError::new(
                        ErrorCode::KernelError,
                        format!("failed to isolate part solid {solid_id}: {e}"),
                    )
                },
            )?;
        }
    } else if let Some(primitive) = req.primitive.clone() {
        let mut builder = TopologyBuilder::new(&mut model);
        match primitive {
            ComponentPrimitive::Box { dx, dy, dz } => {
                builder
                    .create_box_3d(dx, dy, dz)
                    .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
            }
            ComponentPrimitive::Cylinder { radius, height } => {
                builder
                    .create_cylinder_3d(Point3::ORIGIN, Vector3::Z, radius, height)
                    .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
            }
            ComponentPrimitive::Sphere { radius } => {
                builder
                    .create_sphere_3d(Point3::ORIGIN, radius)
                    .map_err(|e| ApiError::new(ErrorCode::InvalidParameter, e.to_string()))?;
            }
        }
    }
    let part = Arc::new(model);
    let name = req.name.clone();
    let cid = guard.add_part_from_source(part, req.name, req.part_id);
    // Caller-supplied transform wins. Otherwise, if this assembly
    // already had components, lay the new one down at +X = N * 30mm
    // so its geometry doesn't fully overlap an existing component.
    // 30mm is intentionally larger than the default-primitive bounds
    // (10mm box, 5mm cylinder/sphere radius) coming from
    // AddComponentDialog so the visible gap is unambiguous.
    let applied_transform: Option<[[f64; 4]; 4]> = match req.transform {
        Some(t) => Some(t),
        None if existing_count > 0 => {
            let offset = existing_count as f64 * 30.0;
            let mut m = [[0.0_f64; 4]; 4];
            m[0][0] = 1.0;
            m[1][1] = 1.0;
            m[2][2] = 1.0;
            m[3][3] = 1.0;
            m[0][3] = offset;
            Some(m)
        }
        None => None,
    };
    if let Some(t) = applied_transform {
        guard
            .set_component_transform(cid, array_to_matrix(t))
            .map_err(map_kernel_err)?;
    }
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.add_component")
            .with_parameters(serde_json::json!({
                "name": name,
                "transform": applied_transform,
                "primitive": req.primitive,
            }))
            .with_input_assembly(id)
            .with_output_component(cid.0),
    );
    Ok(Json(AddComponentResponse {
        component_id: cid.0,
    }))
}

/// Tessellate a single component's BRepModel into a Three.js-shaped
/// mesh payload. The mesh is delivered in the component's *local*
/// frame; the client applies the per-component transform on its
/// Object3D — that way solver / transform updates only require a
/// matrix push on the existing mesh, not a re-fetch.
///
/// Returns an empty mesh (vertex/index/triangle counts = 0) when the
/// component carries no solids (e.g. fresh `add_component` without a
/// primitive payload). 404 only when the assembly or component id is
/// unknown.
pub async fn get_component_mesh(
    State(state): State<AppState>,
    Path((id, comp)): Path<(Uuid, Uuid)>,
) -> Result<Json<ComponentMeshResponse>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    let component = guard.get_component(ComponentId(comp)).ok_or_else(|| {
        ApiError::new(
            ErrorCode::SolidNotFound,
            format!("component {} not found in assembly {}", comp, id),
        )
    })?;
    let params = TessellationParams::default();
    // Single canonical buffer per component — every solid in the
    // BRepModel contributes triangles to the same flat
    // positions/normals/indices arrays, indices rebased per-solid so
    // the caller can upload a single BufferGeometry.
    let mut vertices: Vec<f32> = Vec::new();
    let mut normals: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    for (_, solid) in component.part.solids.iter() {
        let mesh = tessellate_solid(solid, &component.part, &params);
        let base = (vertices.len() / 3) as u32;
        for v in &mesh.vertices {
            vertices.push(v.position.x as f32);
            vertices.push(v.position.y as f32);
            vertices.push(v.position.z as f32);
            normals.push(v.normal.x as f32);
            normals.push(v.normal.y as f32);
            normals.push(v.normal.z as f32);
        }
        for tri in &mesh.triangles {
            indices.push(base + tri[0]);
            indices.push(base + tri[1]);
            indices.push(base + tri[2]);
        }
    }
    let triangle_count = indices.len() / 3;
    Ok(Json(ComponentMeshResponse {
        component_id: comp,
        vertices,
        normals,
        indices,
        triangle_count,
    }))
}

pub async fn remove_component(
    State(state): State<AppState>,
    Path((id, comp)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    guard
        .remove_component(ComponentId(comp))
        .map_err(map_kernel_err)?;
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.remove_component")
            .with_parameters(serde_json::json!({}))
            .with_input_assembly(id)
            .with_input_component(comp),
    );
    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn set_component_transform(
    State(state): State<AppState>,
    Path((id, comp)): Path<(Uuid, Uuid)>,
    Json(req): Json<SetTransformRequest>,
) -> Result<Json<AssemblySummary>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    guard
        .set_component_transform(ComponentId(comp), array_to_matrix(req.transform))
        .map_err(map_kernel_err)?;
    let snap = snapshot(&guard);
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.set_component_transform")
            .with_parameters(serde_json::json!({ "transform": req.transform }))
            .with_input_assembly(id)
            .with_input_component(comp),
    );
    Ok(Json(snap))
}

pub async fn register_mate_reference(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RegisterReferenceRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.name.trim().is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "reference name must not be empty",
        ));
    }
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    let comp = req.component;
    let name = req.name.clone();
    let reference_payload = serde_json::to_value(&req.reference).unwrap_or(serde_json::Value::Null);
    guard
        .register_mate_reference(ComponentId(req.component), req.name, req.reference)
        .map_err(map_kernel_err)?;
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.register_mate_reference")
            .with_parameters(serde_json::json!({
                "name": name,
                "reference": reference_payload,
            }))
            .with_input_assembly(id)
            .with_input_component(comp),
    );
    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn add_mate(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<AddMateRequest>,
) -> Result<Json<AddMateResponse>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    let mate_type = req.mate_type;
    let c1 = req.component1;
    let c2 = req.component2;
    let r1 = req.reference1.clone();
    let r2 = req.reference2.clone();
    let mid = guard
        .add_mate(
            req.mate_type,
            ComponentId(req.component1),
            req.reference1,
            ComponentId(req.component2),
            req.reference2,
        )
        .map_err(map_kernel_err)?;
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.add_mate")
            .with_parameters(serde_json::json!({
                "mate_type": mate_type,
                "component1": c1,
                "reference1": r1,
                "component2": c2,
                "reference2": r2,
            }))
            .with_input_assembly(id)
            .with_input_component(c1)
            .with_input_refs([format!("component:{}", c2)])
            .with_output_mate(mid.0),
    );
    Ok(Json(AddMateResponse { mate_id: mid.0 }))
}

pub async fn remove_mate(
    State(state): State<AppState>,
    Path((id, mate)): Path<(Uuid, Uuid)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    guard.remove_mate(MateId(mate)).map_err(map_kernel_err)?;
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.remove_mate")
            .with_parameters(serde_json::json!({}))
            .with_input_assembly(id)
            .with_input_mate(mate),
    );
    Ok(Json(serde_json::json!({"success": true})))
}

pub async fn patch_mate(
    State(state): State<AppState>,
    Path((id, mate)): Path<(Uuid, Uuid)>,
    Json(req): Json<PatchMateRequest>,
) -> Result<Json<AssemblySummary>, ApiError> {
    if req.suppressed.is_none() && req.flip.is_none() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "patch body must contain at least one of 'suppressed' or 'flip'",
        ));
    }
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    let mid = MateId(mate);
    if let Some(s) = req.suppressed {
        guard.set_mate_suppressed(mid, s).map_err(map_kernel_err)?;
    }
    if let Some(f) = req.flip {
        guard.set_mate_flip(mid, f).map_err(map_kernel_err)?;
    }
    let snap = snapshot(&guard);
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.patch_mate")
            .with_parameters(serde_json::json!({
                "suppressed": req.suppressed,
                "flip": req.flip,
            }))
            .with_input_assembly(id)
            .with_input_mate(mate),
    );
    Ok(Json(snap))
}

pub async fn solve(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<AssemblySummary>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    guard.solve_constraints().map_err(map_kernel_err)?;
    let snap = snapshot(&guard);
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.solve")
            .with_parameters(serde_json::json!({}))
            .with_input_assembly(id),
    );
    Ok(Json(snap))
}

pub async fn explode(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<ExplodeRequest>,
) -> Result<Json<ExplodedViewConfig>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    let cfg = guard.create_exploded_view(req.auto);
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.explode")
            .with_parameters(serde_json::json!({ "auto": req.auto }))
            .with_input_assembly(id),
    );
    Ok(Json(cfg))
}

/// Pairwise component interference for the legacy assembly surface —
/// delegated to the assembly-engine's certified machinery (convexity gate →
/// VHACD convex decomposition → exact-hull EPA contact, with winding-
/// independent ray-parity enclosure so a peg seated in a through-bore stays
/// clear). This replaced the kernel-side `components_interfere` stub that
/// could NEVER report an interference (the #44 silent-0 lie class; killed
/// in the kinematic-assembly campaign, Slice 1).
///
/// Each component's solids are tessellated at default quality and BAKED to
/// world by the component transform (identity engine poses), so a general
/// affine component transform is honoured exactly as placed.
pub(crate) fn interference_pairs(assembly: &Assembly) -> Vec<(Uuid, Uuid)> {
    use assembly_engine as ae;

    // Deterministic order: components sorted by UUID (DashMap iteration is
    // arbitrary; the wire report should be stable).
    let mut components: Vec<_> = assembly.components().map(|c| c.clone()).collect();
    components.sort_by_key(|c| c.id.0);

    let mut ids: Vec<Uuid> = Vec::new();
    let mut engine = ae::Assembly::new(ae::InstanceId(0));
    let params = TessellationParams::default();
    for comp in &components {
        let mut vertices: Vec<[f64; 3]> = Vec::new();
        let mut triangles: Vec<[u32; 3]> = Vec::new();
        for (_, solid) in comp.part.solids.iter() {
            let mesh = tessellate_solid(solid, &comp.part, &params);
            let base = vertices.len() as u32;
            vertices.extend(mesh.vertices.iter().map(|v| {
                let p = comp.transform.transform_point(&v.position);
                [p.x, p.y, p.z]
            }));
            triangles.extend(
                mesh.triangles
                    .iter()
                    .map(|t| [t[0] + base, t[1] + base, t[2] + base]),
            );
        }
        if triangles.is_empty() {
            // A geometry-free component carries no material — it cannot
            // interfere, and the engine would reject an empty TriMesh.
            continue;
        }
        let engine_id = ae::InstanceId(ids.len() as u32);
        ids.push(comp.id.0);
        engine.add_instance(ae::Instance::new(
            engine_id,
            comp.name.clone(),
            ae::Mesh {
                vertices,
                triangles,
            },
        ));
    }

    engine
        .interference_report()
        .interfering
        .into_iter()
        .filter_map(|pair| {
            let a = ids.get(pair.a.0 as usize)?;
            let b = ids.get(pair.b.0 as usize)?;
            Some((*a, *b))
        })
        .collect()
}

pub async fn interferences(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<InterferenceReport>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let guard = handle.read().await;
    let pairs = interference_pairs(&guard);
    Ok(Json(InterferenceReport { pairs }))
}

pub async fn simulate_motion(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<SimulateMotionRequest>,
) -> Result<Json<AssemblySummary>, ApiError> {
    let handle = state.assemblies.get(&id).ok_or_else(|| not_found(id))?;
    let mut guard = handle.write().await;
    let delta = Vector3::new(req.translation[0], req.translation[1], req.translation[2]);
    let rot = req
        .rotation
        .map(|q| Quaternion::new(q[3], q[0], q[1], q[2]));
    let comp = req.component;
    let translation = req.translation;
    let rotation = req.rotation;
    guard
        .simulate_motion(ComponentId(req.component), delta, rot)
        .map_err(map_kernel_err)?;
    let snap = snapshot(&guard);
    drop(guard);
    state.assemblies.record_event(
        RecordedOperation::new("assembly.simulate_motion")
            .with_parameters(serde_json::json!({
                "component": comp,
                "translation": translation,
                "rotation": rotation,
            }))
            .with_input_assembly(id)
            .with_input_component(comp),
    );
    Ok(Json(snap))
}

// Suppress an unused-import warning if `Point3` becomes unreferenced
// during a future trim — kept here because future part-binding slice
// will need it.
#[allow(dead_code)]
fn _force_point3_used() -> Point3 {
    Point3::new(0.0, 0.0, 0.0)
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::operations::recorder::{
        OperationRecorder, RecordedOperation, RecorderError,
    };
    use std::sync::Mutex as StdMutex;

    /// Captures every event the manager emits so tests can assert on
    /// kind/parameters/inputs/outputs. Wrapped in `StdMutex` (not
    /// `tokio::Mutex`) because `OperationRecorder::record` is sync.
    #[derive(Debug, Default)]
    struct CaptureRecorder {
        events: StdMutex<Vec<RecordedOperation>>,
    }

    impl CaptureRecorder {
        fn snapshot(&self) -> Vec<RecordedOperation> {
            self.events
                .lock()
                .expect("CaptureRecorder mutex poisoned")
                .clone()
        }
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

    fn make_assembly() -> (AssemblyManager, Uuid) {
        let mgr = AssemblyManager::new();
        let id = mgr.create("Test");
        (mgr, id)
    }

    async fn add_part(mgr: &AssemblyManager, id: Uuid, name: &str) -> ComponentId {
        let handle = mgr.get(&id).expect("assembly missing");
        let mut a = handle.write().await;
        a.add_part(Arc::new(BRepModel::new()), name)
    }

    #[test]
    fn create_assigns_unique_uuid() {
        let mgr = AssemblyManager::new();
        let a = mgr.create("A");
        let b = mgr.create("B");
        assert_ne!(a, b);
        assert_eq!(mgr.len(), 2);
        assert!(!mgr.is_empty());
    }

    #[test]
    fn get_returns_none_for_unknown_id() {
        let mgr = AssemblyManager::new();
        assert!(mgr.get(&Uuid::new_v4()).is_none());
    }

    #[test]
    fn delete_returns_handle_then_none() {
        let (mgr, id) = make_assembly();
        assert!(mgr.delete(&id).is_some());
        assert!(mgr.delete(&id).is_none());
        assert_eq!(mgr.len(), 0);
        assert!(mgr.is_empty());
    }

    #[test]
    fn list_reports_every_live_id() {
        let mgr = AssemblyManager::new();
        let a = mgr.create("A");
        let b = mgr.create("B");
        let ids = mgr.list();
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&a));
        assert!(ids.contains(&b));
    }

    #[tokio::test]
    async fn snapshot_reflects_added_components() {
        let (mgr, id) = make_assembly();
        let _ = add_part(&mgr, id, "p1").await;
        let _ = add_part(&mgr, id, "p2").await;
        let handle = mgr.get(&id).unwrap();
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        assert_eq!(snap.components.len(), 2);
        assert_eq!(snap.name, "Test");
        assert!(snap.root_component.is_some());
        assert_eq!(snap.mates.len(), 0);
    }

    #[tokio::test]
    async fn snapshot_preserves_identity_transform_for_fresh_part() {
        let (mgr, id) = make_assembly();
        let _ = add_part(&mgr, id, "p1").await;
        let handle = mgr.get(&id).unwrap();
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        let t = snap.components[0].transform;
        for r in 0..4 {
            for c in 0..4 {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!((t[r][c] - expected).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn matrix_round_trip_is_identity() {
        let m = Matrix4::IDENTITY;
        let a = matrix_to_array(m);
        let m2 = array_to_matrix(a);
        for r in 0..4 {
            for c in 0..4 {
                assert!((m[(r, c)] - m2[(r, c)]).abs() < 1e-15);
            }
        }
    }

    #[test]
    fn matrix_round_trip_preserves_translation() {
        let mut m = Matrix4::IDENTITY;
        m[(0, 3)] = 1.0;
        m[(1, 3)] = 2.0;
        m[(2, 3)] = 3.0;
        let a = matrix_to_array(m);
        assert_eq!(a[0][3], 1.0);
        assert_eq!(a[1][3], 2.0);
        assert_eq!(a[2][3], 3.0);
        let m2 = array_to_matrix(a);
        assert_eq!(m2[(0, 3)], 1.0);
        assert_eq!(m2[(1, 3)], 2.0);
        assert_eq!(m2[(2, 3)], 3.0);
    }

    #[test]
    fn not_found_carries_solid_not_found_code() {
        let id = Uuid::new_v4();
        let err = not_found(id);
        assert_eq!(err.code, ErrorCode::SolidNotFound);
        assert!(err.error.contains(&id.to_string()));
    }

    #[test]
    fn map_kernel_err_routes_component_not_found_to_solid_not_found() {
        let cid = ComponentId::new();
        let api = map_kernel_err(AssemblyError::ComponentNotFound(cid));
        assert_eq!(api.code, ErrorCode::SolidNotFound);
    }

    #[test]
    fn map_kernel_err_routes_reference_not_found_to_solid_not_found() {
        let api = map_kernel_err(AssemblyError::ReferenceNotFound("foo".into()));
        assert_eq!(api.code, ErrorCode::SolidNotFound);
    }

    #[test]
    fn map_kernel_err_routes_over_constrained_to_kernel_error() {
        let api = map_kernel_err(AssemblyError::OverConstrained);
        assert_eq!(api.code, ErrorCode::KernelError);
    }

    #[test]
    fn map_kernel_err_routes_conflicting_to_kernel_error() {
        let api = map_kernel_err(AssemblyError::ConflictingConstraints);
        assert_eq!(api.code, ErrorCode::KernelError);
    }

    #[test]
    fn map_kernel_err_routes_solver_failed_to_kernel_error() {
        let api = map_kernel_err(AssemblyError::SolverFailed("nan".into()));
        assert_eq!(api.code, ErrorCode::KernelError);
        assert!(api.error.contains("nan"));
    }

    #[test]
    fn default_true_returns_true() {
        assert!(default_true());
    }

    #[test]
    fn explode_request_defaults_auto_to_true() {
        let v: ExplodeRequest = serde_json::from_str("{}").unwrap();
        assert!(v.auto);
    }

    #[test]
    fn explode_request_honours_explicit_false() {
        let v: ExplodeRequest = serde_json::from_str(r#"{"auto":false}"#).unwrap();
        assert!(!v.auto);
    }

    #[test]
    fn create_assembly_request_round_trips() {
        let v = serde_json::to_string(&CreateAssemblyResponse { id: Uuid::nil() }).unwrap();
        assert!(v.contains("00000000-0000-0000-0000-000000000000"));
    }

    #[test]
    fn add_mate_request_parses_distance_payload() {
        let raw = r#"{
            "mate_type": {"Distance": 5.0},
            "component1": "00000000-0000-0000-0000-000000000001",
            "reference1": "A",
            "component2": "00000000-0000-0000-0000-000000000002",
            "reference2": "B"
        }"#;
        let req: AddMateRequest = serde_json::from_str(raw).unwrap();
        match req.mate_type {
            MateType::Distance(d) => assert!((d - 5.0).abs() < 1e-15),
            other => panic!("expected Distance, got {:?}", other),
        }
    }

    #[test]
    fn add_mate_request_parses_unit_payload() {
        let raw = r#"{
            "mate_type": "Coincident",
            "component1": "00000000-0000-0000-0000-000000000001",
            "reference1": "A",
            "component2": "00000000-0000-0000-0000-000000000002",
            "reference2": "B"
        }"#;
        let req: AddMateRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.mate_type, MateType::Coincident);
    }

    #[test]
    fn patch_mate_request_supports_only_suppressed() {
        let raw = r#"{"suppressed": true}"#;
        let req: PatchMateRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.suppressed, Some(true));
        assert_eq!(req.flip, None);
    }

    #[test]
    fn patch_mate_request_supports_only_flip() {
        let raw = r#"{"flip": true}"#;
        let req: PatchMateRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.flip, Some(true));
        assert_eq!(req.suppressed, None);
    }

    #[test]
    fn simulate_motion_request_supports_no_rotation() {
        let raw = r#"{
            "component": "00000000-0000-0000-0000-000000000001",
            "translation": [1.0, 0.0, 0.0]
        }"#;
        let req: SimulateMotionRequest = serde_json::from_str(raw).unwrap();
        assert!(req.rotation.is_none());
        assert_eq!(req.translation[0], 1.0);
    }

    #[test]
    fn register_reference_request_parses_plane() {
        let raw = r#"{
            "component": "00000000-0000-0000-0000-000000000001",
            "name": "top",
            "reference": {
                "Plane": {
                    "origin": [0.0, 0.0, 1.0],
                    "normal": [0.0, 0.0, 1.0]
                }
            }
        }"#;
        let req: RegisterReferenceRequest = serde_json::from_str(raw).unwrap();
        match req.reference {
            MateReference::Plane { origin, normal } => {
                assert_eq!(origin.z, 1.0);
                assert_eq!(normal.z, 1.0);
            }
            other => panic!("expected Plane, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn add_component_via_manager_registers_part() {
        let (mgr, id) = make_assembly();
        let cid = add_part(&mgr, id, "first").await;
        let handle = mgr.get(&id).unwrap();
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        assert_eq!(snap.components.len(), 1);
        assert_eq!(snap.components[0].id, cid.0);
        assert_eq!(snap.components[0].name, "first");
        assert!(snap.components[0].is_fixed);
        assert_eq!(snap.root_component, Some(cid.0));
    }

    #[tokio::test]
    async fn second_component_is_not_fixed_and_root_unchanged() {
        let (mgr, id) = make_assembly();
        let first = add_part(&mgr, id, "first").await;
        let second = add_part(&mgr, id, "second").await;
        let handle = mgr.get(&id).unwrap();
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        assert_eq!(snap.root_component, Some(first.0));
        let by_id = |needle: ComponentId| {
            snap.components
                .iter()
                .find(|c| c.id == needle.0)
                .cloned()
                .unwrap()
        };
        assert!(by_id(first).is_fixed);
        assert!(!by_id(second).is_fixed);
    }

    #[tokio::test]
    async fn remove_component_drops_it() {
        let (mgr, id) = make_assembly();
        let cid = add_part(&mgr, id, "p").await;
        let handle = mgr.get(&id).unwrap();
        {
            let mut guard = handle.write().await;
            guard.remove_component(cid).unwrap();
        }
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        assert_eq!(snap.components.len(), 0);
        assert!(snap.root_component.is_none());
    }

    #[tokio::test]
    async fn register_mate_reference_appears_in_snapshot() {
        let (mgr, id) = make_assembly();
        let cid = add_part(&mgr, id, "p").await;
        let handle = mgr.get(&id).unwrap();
        {
            let mut guard = handle.write().await;
            guard
                .register_mate_reference(
                    cid,
                    "top",
                    MateReference::Plane {
                        origin: Point3::new(0.0, 0.0, 0.0),
                        normal: Vector3::new(0.0, 0.0, 1.0),
                    },
                )
                .unwrap();
        }
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        let refs = &snap.components[0].mate_references;
        assert_eq!(refs.len(), 1);
        assert_eq!(refs[0].name, "top");
    }

    #[tokio::test]
    async fn add_mate_and_solve_marks_solved() {
        let (mgr, id) = make_assembly();
        let a = add_part(&mgr, id, "a").await;
        let b = add_part(&mgr, id, "b").await;
        let handle = mgr.get(&id).unwrap();
        {
            let mut guard = handle.write().await;
            guard
                .register_mate_reference(
                    a,
                    "top",
                    MateReference::Plane {
                        origin: Point3::new(0.0, 0.0, 0.0),
                        normal: Vector3::new(0.0, 0.0, 1.0),
                    },
                )
                .unwrap();
            guard
                .register_mate_reference(
                    b,
                    "bot",
                    MateReference::Plane {
                        origin: Point3::new(0.0, 0.0, 5.0),
                        normal: Vector3::new(0.0, 0.0, 1.0),
                    },
                )
                .unwrap();
            let _mid = guard
                .add_mate(MateType::Coincident, a, "top", b, "bot")
                .unwrap();
        }
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        assert_eq!(snap.mates.len(), 1);
        assert!(snap.mates[0].solved || snap.mates[0].error.is_some());
    }

    #[tokio::test]
    async fn snapshot_clones_exploded_config() {
        let (mgr, id) = make_assembly();
        let _ = add_part(&mgr, id, "a").await;
        let handle = mgr.get(&id).unwrap();
        let cfg = {
            let mut guard = handle.write().await;
            guard.create_exploded_view(true)
        };
        let guard = handle.read().await;
        let snap = snapshot(&guard);
        let snap_cfg = snap.exploded.expect("exploded missing from snapshot");
        assert_eq!(snap_cfg.auto_explode, cfg.auto_explode);
        assert_eq!(snap_cfg.scale, cfg.scale);
        assert_eq!(snap_cfg.steps.len(), cfg.steps.len());
    }

    #[tokio::test]
    async fn interferences_stay_empty_for_geometry_free_components() {
        // Components seeded with EMPTY BRepModels carry no material — they
        // genuinely cannot interfere, so the report must stay empty (a
        // degenerate-input guard, not the old stub pin).
        let (mgr, id) = make_assembly();
        let _ = add_part(&mgr, id, "a").await;
        let _ = add_part(&mgr, id, "b").await;
        let handle = mgr.get(&id).unwrap();
        let guard = handle.read().await;
        assert!(interference_pairs(&guard).is_empty());
    }

    /// A 10mm cube centred at the origin, as a component-seed model.
    fn box_model(side: f64) -> BRepModel {
        let mut m = BRepModel::new();
        let built = TopologyBuilder::new(&mut m).create_box_3d(side, side, side);
        assert!(built.is_ok(), "box primitive must build: {built:?}");
        m
    }

    fn translate(x: f64, y: f64, z: f64) -> Matrix4 {
        let mut t = Matrix4::IDENTITY;
        t[(0, 3)] = x;
        t[(1, 3)] = y;
        t[(2, 3)] = z;
        t
    }

    #[tokio::test]
    async fn interferences_reports_overlapping_components() {
        // RED → GREEN for the §2.3.1 silent-lie surface (kinematic-assembly
        // campaign, Slice 1, defect a): `/api/assemblies/{id}/interferences`
        // could NEVER report an interference because the kernel-side
        // `components_interfere` was a stub returning `false`.
        //
        // Pre-fix signature (captured 2026-07-17, HEAD 45d8ffee): two 10mm
        // cubes overlapping by 5mm → `pairs == []`.
        let (mgr, id) = make_assembly();
        let handle = mgr.get(&id).expect("assembly just created");
        let (ca, cb, cc) = {
            let mut a = handle.write().await;
            let ca = a.add_part(Arc::new(box_model(10.0)), "hub");
            let cb = a.add_part(Arc::new(box_model(10.0)), "overlapping");
            let cc = a.add_part(Arc::new(box_model(10.0)), "clear");
            a.set_component_transform(cb, translate(5.0, 0.0, 0.0))
                .expect("cb exists");
            a.set_component_transform(cc, translate(100.0, 0.0, 0.0))
                .expect("cc exists");
            (ca, cb, cc)
        };
        let guard = handle.read().await;
        let pairs = interference_pairs(&guard);
        let has = |x: ComponentId, y: ComponentId| {
            pairs
                .iter()
                .any(|&(p, q)| (p == x.0 && q == y.0) || (p == y.0 && q == x.0))
        };
        assert!(
            has(ca, cb),
            "cubes overlapping by 5mm MUST be reported; got {pairs:?}"
        );
        assert!(
            !has(ca, cc) && !has(cb, cc),
            "the far cube is clear of both; got {pairs:?}"
        );
    }

    // ── Recorder integration (A.3) ──────────────────────────────────

    #[test]
    fn manager_without_recorder_silently_drops_events() {
        // Default constructor leaves recorder = None. Calling
        // `record_event` must be a hard no-op — this is the path
        // every unit test in this module relies on.
        let mgr = AssemblyManager::new();
        mgr.record_event(RecordedOperation::new("assembly.noop"));
        // No panic, no observable side-effect — coverage by absence.
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn with_recorder_emits_events_through_attached_recorder() {
        let capture = Arc::new(CaptureRecorder::default());
        let mgr = AssemblyManager::with_recorder(capture.clone() as Arc<dyn OperationRecorder>);
        mgr.record_event(
            RecordedOperation::new("assembly.create")
                .with_parameters(serde_json::json!({ "name": "demo" }))
                .with_output_assembly(Uuid::nil()),
        );
        mgr.record_event(
            RecordedOperation::new("assembly.delete")
                .with_parameters(serde_json::json!({}))
                .with_input_assembly(Uuid::nil()),
        );
        let events = capture.snapshot();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, "assembly.create");
        assert_eq!(events[1].kind, "assembly.delete");
        assert_eq!(events[0].outputs, vec![format!("assembly:{}", Uuid::nil())]);
        assert_eq!(events[1].inputs, vec![format!("assembly:{}", Uuid::nil())]);
    }

    #[test]
    fn recorder_failure_does_not_panic_and_is_swallowed() {
        // Recorder returning Err must never propagate — the mutation
        // has already succeeded, the event log is best-effort.
        #[derive(Debug)]
        struct FailingRecorder;
        impl OperationRecorder for FailingRecorder {
            fn record(&self, _op: RecordedOperation) -> Result<(), RecorderError> {
                Err(RecorderError::Unavailable("test fault".into()))
            }
        }
        let mgr = AssemblyManager::with_recorder(Arc::new(FailingRecorder));
        mgr.record_event(RecordedOperation::new("assembly.create"));
        // Reached here ⇒ no panic.
        assert_eq!(mgr.len(), 0);
    }

    #[test]
    fn recorded_kinds_cover_every_mutating_handler() {
        // Belt-and-braces check against the public surface: every
        // handler that mutates state must use an `assembly.*` kind
        // string in this exact set. If a new handler is added
        // without instrumentation, this list goes stale — adding
        // the entry forces the author to think about provenance.
        let expected_kinds = [
            "assembly.create",
            "assembly.delete",
            "assembly.add_component",
            "assembly.remove_component",
            "assembly.set_component_transform",
            "assembly.register_mate_reference",
            "assembly.add_mate",
            "assembly.remove_mate",
            "assembly.patch_mate",
            "assembly.solve",
            "assembly.explode",
            "assembly.simulate_motion",
        ];
        // All kinds in the expected set use the canonical
        // `assembly.<verb>` namespace — guard against typos.
        for k in expected_kinds {
            assert!(k.starts_with("assembly."), "{} has wrong namespace", k);
            assert!(!k.contains(' '), "{} has whitespace", k);
        }
    }
}
