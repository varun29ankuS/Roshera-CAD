//! Agent-readable REST surface — datum 6.
//!
//! Thin Axum wrappers around the kernel's `readable/` query module
//! (geometry-engine `src/readable/`). Each handler maps directly onto
//! one method of [`BRepModel`]; the kernel is the single source of
//! truth, and this layer adds only the HTTP framing, error mapping,
//! and read-vs-write lock discipline.
//!
//! ## Why a dedicated handler module
//! These endpoints serve agents (LLM tools, external scripts) reading
//! a model. They differ from the existing `geometry` / `datums`
//! handlers in two ways:
//!  1. **Verb-rich URLs.** Agents address geometry by *part identity +
//!     anchor datum* (per the readable-module thesis), so the routes
//!     read like queries — `GET /api/agent/parts`,
//!     `GET /api/agent/parts/{id}/mass` — rather than CRUD.
//!  2. **Wire shapes are the kernel report types.** No DTO translation
//!     layer: the agent gets `PartReport`, `PartSummary`,
//!     `DatumSummary`, etc. exactly as the kernel produces them. Drift
//!     between kernel and wire is impossible because there is no
//!     middle representation.
//!
//! ## Lock discipline
//! Read-only queries (`list_parts`, `query_part`, `parts_near_datum`,
//! `part_distance`, `list_datums`) hold a single `model.read().await`.
//! Cache-warming queries (`mass_properties_for`, `oriented_bbox_for`,
//! `query_face`, `query_edge`) and the mutating `reanchor_part`
//! upgrade to `model.write().await` — the underlying kernel methods
//! take `&mut self` because computing volume / area / length the first
//! time populates a per-entity cache. Subsequent calls are O(1) and
//! still need `&mut`, so the lock cost is paid once per entity per
//! process lifetime.

use crate::part_mgr::ActiveModel;
use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use geometry_engine::math::Matrix4;
use geometry_engine::primitives::datum::DatumId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::readable::{
    DatumSummary, DistanceReport, EdgeReport, FaceReport, HoverReport, ListPartsFilter,
    MassPropertiesReport, OrientedBBox, PartProximity, PartReport, PartSummary,
};
use serde::{Deserialize, Serialize};

// ───────────────────── parts ────────────────────────────────────────

/// Query parameters for `GET /api/agent/parts`. Both filters are
/// optional and AND-ed by the kernel (matches `ListPartsFilter`'s
/// semantics). An empty query returns every solid in the model.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ListPartsQuery {
    /// Restrict to parts whose anchor datum equals this id.
    pub anchor_datum_id: Option<DatumId>,
    /// Case-insensitive substring match against the part's name.
    pub name_contains: Option<String>,
}

/// `GET /api/agent/parts` — list every part as a [`PartSummary`].
///
/// Optional filters: `?anchor_datum_id=N`, `?name_contains=foo`.
/// Filters are AND-ed; an empty query returns the entire model.
pub async fn list_parts(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Query(q): Query<ListPartsQuery>,
) -> Json<Vec<PartSummary>> {
    let model = model_handle.read().await;
    let filter = ListPartsFilter {
        anchor_datum_id: q.anchor_datum_id,
        name_contains: q.name_contains,
    };
    Json(model.list_parts_filtered(&filter))
}

/// `GET /api/agent/parts/{id}` — full agent-facing report for a single
/// part. `404` when the id is unknown or the solid is degenerate.
///
/// Cache-warming on first call — takes a write lock because the kernel
/// drives `Solid::compute_mass_properties` to populate the cached
/// volume / surface-area / centre-of-mass figures stamped into the
/// returned [`PartReport`]. Subsequent calls hit the per-solid cache.
/// Same pattern as `part_mass_properties`.
pub async fn query_part(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
) -> Result<Json<PartReport>, StatusCode> {
    let mut model = model_handle.write().await;
    model.query_part(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/{id}/mass` — mass properties (volume, mass,
/// COG, inertia tensor, principal axes, radius of gyration).
///
/// Cache-warming on first call — takes a write lock because the kernel
/// populates per-entity caches during the divergence-theorem integral.
pub async fn part_mass_properties(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
) -> Result<Json<MassPropertiesReport>, StatusCode> {
    let mut model = model_handle.write().await;
    model
        .mass_properties_for(id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/uuid/{uuid}/mass` — UUID-keyed wrapper around
/// [`part_mass_properties`]. The frontend addresses objects by UUID
/// (the wire `id` of every CAD object); this resolves the UUID to its
/// kernel `SolidId` via [`AppState::get_local_id`] and dispatches to
/// the same mass-properties path.
pub async fn part_mass_properties_by_uuid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(uuid): Path<uuid::Uuid>,
) -> Result<Json<MassPropertiesReport>, StatusCode> {
    let solid_id = state.get_local_id(&uuid).ok_or(StatusCode::NOT_FOUND)?;
    let mut model = model_handle.write().await;
    model
        .mass_properties_for(solid_id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/{id}/obb` — oriented bounding box (axes
/// aligned to the part's principal moments of inertia).
pub async fn part_oriented_bbox(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
) -> Result<Json<OrientedBBox>, StatusCode> {
    let mut model = model_handle.write().await;
    model
        .oriented_bbox_for(id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/distance/{a}/{b}` — bbox-center, AABB-gap,
/// overlap, and direction unit vector between two parts.
pub async fn part_distance(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((a, b)): Path<(SolidId, SolidId)>,
) -> Result<Json<DistanceReport>, StatusCode> {
    let model = model_handle.read().await;
    model
        .part_distance(a, b)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/distance/uuid/{a}/{b}` — UUID-keyed wrapper
/// around [`part_distance`]. Resolves both UUIDs through
/// [`AppState::get_local_id`] before dispatching to the kernel, so
/// frontends that address objects by public UUID don't need their
/// own UUID-to-SolidId resolver.
pub async fn part_distance_by_uuid(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((a_uuid, b_uuid)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<Json<DistanceReport>, StatusCode> {
    let a = state.get_local_id(&a_uuid).ok_or(StatusCode::NOT_FOUND)?;
    let b = state.get_local_id(&b_uuid).ok_or(StatusCode::NOT_FOUND)?;
    let model = model_handle.read().await;
    model
        .part_distance(a, b)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Request body for `POST /api/agent/parts/{id}/reanchor`.
///
/// `new_datum_id` is required; `local_transform`, when omitted, leaves
/// the part's existing local transform untouched (equivalent to
/// re-parenting under a new datum without disturbing placement). When
/// supplied, the row-major 4×4 replaces the existing transform.
#[derive(Debug, Clone, Deserialize)]
pub struct ReanchorPartRequest {
    pub new_datum_id: DatumId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local_transform: Option<[[f64; 4]; 4]>,
}

/// Response body for `POST /api/agent/parts/{id}/reanchor`. Echoes the
/// applied anchor so the agent can confirm without a follow-up query.
#[derive(Debug, Clone, Serialize)]
pub struct ReanchorPartResponse {
    pub solid_id: SolidId,
    pub datum_id: DatumId,
}

/// `POST /api/agent/parts/{id}/reanchor` — re-anchor a part to a
/// different datum, optionally with a new local-frame offset.
///
/// `404` when the solid id or datum id is unknown; `422` when the
/// kernel mediator fails (e.g. transform composition rejected).
pub async fn reanchor_part(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
    Json(req): Json<ReanchorPartRequest>,
) -> Result<Json<ReanchorPartResponse>, StatusCode> {
    use geometry_engine::readable::query::ReanchorError;

    let mut model = model_handle.write().await;
    let offset = req.local_transform.map(Matrix4::from_rows_array);
    match model.reanchor_part(id, req.new_datum_id, offset) {
        Ok(()) => Ok(Json(ReanchorPartResponse {
            solid_id: id,
            datum_id: req.new_datum_id,
        })),
        Err(ReanchorError::UnknownSolid(_)) | Err(ReanchorError::UnknownDatum(_)) => {
            Err(StatusCode::NOT_FOUND)
        }
        Err(ReanchorError::Internal(msg)) => {
            tracing::warn!("reanchor_part failed: {}", msg);
            Err(StatusCode::UNPROCESSABLE_ENTITY)
        }
    }
}

// ───────────────────── datums ───────────────────────────────────────

/// `GET /api/agent/datums` — agent-rich datum list (carries
/// kind/subkind/origin/frame_z/source_kind on every entry).
///
/// Distinct from `GET /api/datums` (the frontend's `DatumDto` list):
/// the agent surface adds `subkind`, `frame_z`, and `source_kind` so
/// an LLM can plan around what each datum actually represents without
/// follow-up queries.
pub async fn list_datums(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
) -> Json<Vec<DatumSummary>> {
    let model = model_handle.read().await;
    Json(model.list_datums())
}

/// Query parameters for `GET /api/agent/datums/{id}/parts`. `radius`
/// is required and must be non-negative; non-finite or negative values
/// produce an empty result set (matches the kernel contract).
#[derive(Debug, Clone, Deserialize)]
pub struct PartsNearDatumQuery {
    pub radius: f64,
}

/// `GET /api/agent/datums/{id}/parts?radius=R` — every part whose bbox
/// center is within `R` of the supplied datum, ordered ascending by
/// distance.
///
/// Distance metric depends on the datum kind: Euclidean for `Origin`,
/// perpendicular for `Plane(_)`, line-distance for `Axis(_)`. See
/// `BRepModel::parts_near_datum`.
pub async fn parts_near_datum(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<DatumId>,
    Query(q): Query<PartsNearDatumQuery>,
) -> Json<Vec<PartProximity>> {
    let model = model_handle.read().await;
    Json(model.parts_near_datum(id, q.radius))
}

// ───────────────────── faces & edges ────────────────────────────────

/// `GET /api/agent/faces/{id}` — per-face report (surface type, area,
/// boundary edges, neighbouring faces, host solid).
pub async fn query_face(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<FaceReport>, StatusCode> {
    let mut model = model_handle.write().await;
    model.query_face(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/edges/{id}` — per-edge report (curve kind, length,
/// start/end vertex world coordinates).
pub async fn query_edge(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<EdgeReport>, StatusCode> {
    let mut model = model_handle.write().await;
    model.query_edge(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/hover/{id}` — resolve a hovered face id into a
/// [`HoverReport`]: the face's report joined with its host part's name and
/// datum-anchored one-liner. The kernel side of the HOVER-α signal pipe —
/// the viewport resolves a raycast to a `FaceId` via the mesh's
/// per-triangle `face_map`, then this turns it into agent-addressable
/// context (which part, anchored where) in one round-trip. Pure query: no
/// model mutation, no timeline event. Cache-warming, so `&mut` like
/// `query_face`. `404` on unknown id.
pub async fn query_hover(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<HoverReport>, StatusCode> {
    let mut model = model_handle.write().await;
    model.query_hover(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

// ───────────────────── render (agent eyes) ──────────────────────────

/// Query parameters for `GET /api/agent/parts/{id}/render`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RenderQuery {
    /// `iso` (default) | `front` | `top` | `right`.
    pub view: Option<String>,
    /// `shaded` (default) | `ids` | `depth` | `normals`.
    pub mode: Option<String>,
    /// Square output size in pixels, clamped to 64..=2048. Default 512.
    pub size: Option<usize>,
}

/// One legend row: which RGB in an `ids`-mode image is which face.
#[derive(Debug, Clone, Serialize)]
pub struct FaceLegendEntry {
    pub face_id: u32,
    pub rgb: [u8; 3],
}

/// Wire shape for a rendered view.
#[derive(Debug, Clone, Serialize)]
pub struct RenderResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub view: String,
    pub mode: String,
    /// Populated in `ids` mode: exact color → face mapping (flat colors,
    /// so the mapping survives image resampling).
    pub face_legend: Vec<FaceLegendEntry>,
    /// `diagnostic` mode: count of OPEN (boundary) mesh edges — missing-face
    /// hole rims, drawn red in the image. 0 in other modes.
    pub open_edges: usize,
    /// `diagnostic` mode: count of NON-MANIFOLD mesh edges (3+ triangles —
    /// overlapping/duplicate faces), drawn magenta. 0 in other modes.
    pub nonmanifold_edges: usize,
}

/// `GET /api/agent/parts/{id}/render` — the agent's eye.
///
/// Renders the solid through the kernel's deterministic software
/// rasterizer (no GPU, no display). `mode=ids` is set-of-marks for
/// topology: every face a distinct flat color plus a legend, so a
/// vision model can ADDRESS what it sees ("the red face is face 12").
/// `depth`/`normals` are the G-buffer channels — exact depth and
/// orientation, no stereo inference needed.
///
/// Read lock only: rendering tessellates into a scratch mesh and never
/// mutates the model. `404` unknown id / empty tessellation, `400`
/// unknown view or mode.
pub async fn render_part(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<RenderQuery>,
) -> Result<Json<RenderResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::render::{render_solid, CanonicalView, RenderMode, RenderOptions};

    let view_name = q.view.as_deref().unwrap_or("iso");
    let view = match view_name {
        "iso" => CanonicalView::Isometric,
        "front" => CanonicalView::Front,
        "top" => CanonicalView::Top,
        "right" => CanonicalView::Right,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let mode_name = q.mode.as_deref().unwrap_or("shaded");
    let mode = match mode_name {
        "shaded" => RenderMode::Shaded,
        "ids" => RenderMode::FaceIds,
        "depth" => RenderMode::Depth,
        "normals" => RenderMode::Normals,
        "diagnostic" => RenderMode::Diagnostic,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let size = q.size.unwrap_or(512).clamp(64, 2048);

    let model = model_handle.read().await;
    let frame = render_solid(
        &model,
        id as SolidId,
        &RenderOptions {
            width: size,
            height: size,
            view,
            mode,
            ..Default::default()
        },
    )
    .ok_or(StatusCode::NOT_FOUND)?;
    let png = frame
        .to_png()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(RenderResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: frame.width,
        height: frame.height,
        view: view_name.to_string(),
        mode: mode_name.to_string(),
        face_legend: frame
            .face_legend
            .iter()
            .map(|&(face_id, rgb)| FaceLegendEntry { face_id, rgb })
            .collect(),
        open_edges: frame.open_edges,
        nonmanifold_edges: frame.nonmanifold_edges,
    }))
}

// ───────────────────── viewpoint selection (EYE-6) ──────────────────

/// `GET /api/agent/parts/{id}/best-view` — EYE-6 active perception.
///
/// Returns the most-informative single view (max viewpoint entropy) plus a
/// greedy next-best-view sequence that covers every face — the answer to
/// EYE-5's "request another angle". `?candidates=N` (default 64) sets the
/// Fibonacci view-sphere density. Read lock only; `404` on unknown id.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BestViewQuery {
    pub candidates: Option<usize>,
}

pub async fn part_best_view(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<BestViewQuery>,
) -> Result<Json<geometry_engine::render::viewpoint::ViewpointReport>, StatusCode> {
    use geometry_engine::render::viewpoint::analyze_viewpoints;
    use geometry_engine::tessellation::TessellationParams;

    let n = q.candidates.unwrap_or(64).clamp(8, 256);
    let model = model_handle.read().await;
    analyze_viewpoints(&model, id as SolidId, n, &TessellationParams::default())
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// Query for `GET /api/agent/parts/{id}/orbit` — render from an arbitrary
/// azimuth/elevation (world Z up). The companion to best-view: once the agent
/// knows where to look, this shows it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OrbitQuery {
    pub az: Option<f64>,
    pub el: Option<f64>,
    pub mode: Option<String>,
    pub size: Option<usize>,
}

/// One arbitrary-direction render + its camera basis (so coordinates stay
/// recoverable from frame + query).
#[derive(Debug, Clone, Serialize)]
pub struct OrbitResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub az_deg: f64,
    pub el_deg: f64,
    pub dir: [f64; 3],
    pub mode: String,
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
}

pub async fn part_orbit(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<OrbitQuery>,
) -> Result<Json<OrbitResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::math::Vector3;
    use geometry_engine::render::{render_solid_dir, CanonicalView, RenderMode, RenderOptions};

    let az = q.az.unwrap_or(45.0).to_radians();
    let el = q.el.unwrap_or(30.0).to_radians();
    // Camera position on the unit sphere (world Z up) → view dir = −position.
    let pos = [el.cos() * az.cos(), el.cos() * az.sin(), el.sin()];
    let dir = Vector3::new(-pos[0], -pos[1], -pos[2]);
    let up_hint = if pos[2].abs() > 0.999 {
        Vector3::new(0.0, 1.0, 0.0)
    } else {
        Vector3::new(0.0, 0.0, 1.0)
    };
    let mode_name = q.mode.as_deref().unwrap_or("shaded");
    let mode = match mode_name {
        "shaded" => RenderMode::Shaded,
        "ids" => RenderMode::FaceIds,
        "depth" => RenderMode::Depth,
        "normals" => RenderMode::Normals,
        "diagnostic" => RenderMode::Diagnostic,
        _ => return Err(StatusCode::BAD_REQUEST),
    };
    let size = q.size.unwrap_or(512).clamp(64, 2048);

    let model = model_handle.read().await;
    let frame = render_solid_dir(
        &model,
        id as SolidId,
        dir,
        up_hint,
        &RenderOptions {
            width: size,
            height: size,
            view: CanonicalView::Isometric, // ignored by render_solid_dir
            mode,
            ..Default::default()
        },
    )
    .ok_or(StatusCode::NOT_FOUND)?;
    let png = frame
        .to_png()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(OrbitResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: frame.width,
        height: frame.height,
        az_deg: q.az.unwrap_or(45.0),
        el_deg: q.el.unwrap_or(30.0),
        dir: [dir.x, dir.y, dir.z],
        mode: mode_name.to_string(),
        open_edges: frame.open_edges,
        nonmanifold_edges: frame.nonmanifold_edges,
    }))
}

// ───────────────────── coverage / ambiguity (EYE-5) ─────────────────

/// `GET /api/agent/parts/{id}/coverage` — EYE-5 honesty protocol.
///
/// Reports which faces the 4 standard views actually show vs leave unseen, so
/// an agent knows when it must request another angle instead of assuming full
/// coverage. Read lock only; `404` on unknown id / empty tessellation.
pub async fn part_coverage(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<geometry_engine::render::dimensioned::CoverageReport>, StatusCode> {
    use geometry_engine::render::dimensioned::coverage_report;
    use geometry_engine::tessellation::TessellationParams;

    let model = model_handle.read().await;
    coverage_report(&model, id as SolidId, &TessellationParams::default())
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

// ───────────────────── features + measure (EYE-4) ───────────────────

/// Wire shape for `GET /api/agent/parts/{id}/features`: every face's analytic
/// feature dimensions plus a distinct-diameter summary.
#[derive(Debug, Clone, Serialize)]
pub struct FeaturesResponse {
    pub features: Vec<geometry_engine::readable::FeatureDim>,
    /// Distinct cylindrical (bore/boss) diameters present, each with a count.
    pub diameters: Vec<DiameterCount>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DiameterCount {
    pub diameter: f64,
    pub count: usize,
}

/// `GET /api/agent/parts/{id}/features` — EYE-4 feature extraction.
///
/// Reads analytic feature sizes straight off each face (cylinder diameters +
/// axes, plane normals) so an agent can ask "what holes/bosses and how big"
/// without measuring pixels. Read lock only; `404` on unknown id.
pub async fn part_features(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<FeaturesResponse>, StatusCode> {
    use geometry_engine::readable::{cylindrical_diameters, extract_features};

    let model = model_handle.read().await;
    if model.solids.get(id as SolidId).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let features = extract_features(&model, id as SolidId);
    let diameters = cylindrical_diameters(&model, id as SolidId)
        .into_iter()
        .map(|(diameter, count)| DiameterCount { diameter, count })
        .collect();
    Ok(Json(FeaturesResponse {
        features,
        diameters,
    }))
}

// ───────────────────── dimensioned multi-view (EYE-1) ───────────────

/// A world-space 3-vector on the wire.
#[derive(Debug, Clone, Serialize)]
pub struct Vec3Wire {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

// Point3 is a type alias for Vector3 in the kernel, so one impl covers both.
impl From<geometry_engine::math::Vector3> for Vec3Wire {
    fn from(v: geometry_engine::math::Vector3) -> Self {
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
        }
    }
}

/// One quadrant's orthographic camera matrix — the structured payload that
/// makes coordinates RECOVERABLE: world `p` → pixel
/// `(ox + (p·right)·scale, oy − (p·up)·scale)` within `cell`. The agent reads
/// geometry from this transform + the frame, never by guessing from pixels.
#[derive(Debug, Clone, Serialize)]
pub struct ViewProjectionWire {
    pub view: String,
    pub label: String,
    pub right: Vec3Wire,
    pub up: Vec3Wire,
    pub dir: Vec3Wire,
    pub scale: f64,
    pub ox: f64,
    pub oy: f64,
    /// (x, y, w, h) of this view's cell within the composite image.
    pub cell: [usize; 4],
}

/// L×W×H extents (mm), X/Y/Z.
#[derive(Debug, Clone, Serialize)]
pub struct DimsWire {
    pub l: f64,
    pub w: f64,
    pub h: f64,
}

/// Wire shape for the dimensioned multi-view render.
#[derive(Debug, Clone, Serialize)]
pub struct DimensionedResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub units: String,
    pub bbox_min: Vec3Wire,
    pub bbox_max: Vec3Wire,
    pub dims: DimsWire,
    pub scale_bar_world: f64,
    pub views: Vec<ViewProjectionWire>,
    /// EYE-3 analytics, measured off the same mesh the views are drawn from:
    /// volume, surface area, and the volume centroid. Match the kernel's
    /// mass-properties query within faceting tolerance (visual⇄numeric check).
    pub volume: f64,
    pub surface_area: f64,
    pub centroid: Vec3Wire,
}

/// `GET /api/agent/parts/{id}/dimensioned` — EYE-1, the measuring eye.
///
/// A 2×2 (Front/Right/Top/Iso) dimensioned composite plus the per-view camera
/// matrices, bbox, and L×W×H — measured off the tessellated mesh, never
/// assumed. The image is the aligned aid; the JSON is authoritative. Read lock
/// only (pure query). `404` unknown id / empty tessellation.
pub async fn render_dimensioned(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<DimensionedResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::render::dimensioned::render_dimensioned_multiview;
    use geometry_engine::render::CanonicalView;
    use geometry_engine::tessellation::TessellationParams;

    let model = model_handle.read().await;
    let frame = render_dimensioned_multiview(&model, id as SolidId, &TessellationParams::default())
        .ok_or(StatusCode::NOT_FOUND)?;
    let png = frame
        .to_png()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let view_name = |v: CanonicalView| -> String {
        match v {
            CanonicalView::Isometric => "iso",
            CanonicalView::Front => "front",
            CanonicalView::Top => "top",
            CanonicalView::Right => "right",
        }
        .to_string()
    };

    Ok(Json(DimensionedResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: frame.width,
        height: frame.height,
        units: frame.units.to_string(),
        bbox_min: frame.bbox_min.into(),
        bbox_max: frame.bbox_max.into(),
        dims: DimsWire {
            l: frame.dims.0,
            w: frame.dims.1,
            h: frame.dims.2,
        },
        scale_bar_world: frame.scale_bar_world,
        views: frame
            .views
            .iter()
            .map(|vp| ViewProjectionWire {
                view: view_name(vp.view),
                label: vp.label.to_string(),
                right: vp.right.into(),
                up: vp.up.into(),
                dir: vp.dir.into(),
                scale: vp.scale,
                ox: vp.ox,
                oy: vp.oy,
                cell: [vp.cell.0, vp.cell.1, vp.cell.2, vp.cell.3],
            })
            .collect(),
        volume: frame.volume,
        surface_area: frame.surface_area,
        centroid: frame.centroid.into(),
    }))
}

// ───────────────────── pointer (the user's attention) ───────────────

/// Wire shape for `GET /api/agent/pointer`: the user's latest viewport
/// interaction joined with the kernel's hover report for the touched
/// face. `hover` is `None` when the pointer event carried no face id
/// or the face has since been consumed by an operation.
#[derive(Debug, Serialize)]
pub struct PointerReport {
    pub event: crate::viewport_bridge::PointerEvent,
    pub hover: Option<HoverReport>,
}

/// `POST /api/agent/pointer` — the viewport reports what the user is
/// pointing at (click or hover-dwell). Fire-and-forget from the
/// frontend; latest-wins storage (attention has no backlog).
pub async fn set_pointer(
    State(state): State<AppState>,
    Json(event): Json<crate::viewport_bridge::PointerEvent>,
) -> StatusCode {
    *state.viewport_bridge.pointer.lock() = Some(event);
    StatusCode::NO_CONTENT
}

/// `GET /api/agent/pointer` — what is the user pointing at right now?
///
/// The HUMAN→AGENT half of shared perception: an agent in conversation
/// reads this to ground deixis ("this hole", "that face") against real
/// topology. Joins the stored pointer event with `query_hover` so the
/// agent gets surface type, area, and host part in one call. `404`
/// when no pointer event has been reported yet.
pub async fn get_pointer(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
) -> Result<Json<PointerReport>, StatusCode> {
    let event = state
        .viewport_bridge
        .pointer
        .lock()
        .clone()
        .ok_or(StatusCode::NOT_FOUND)?;
    let hover = match event.face_id {
        Some(fid) => {
            let mut model = model_handle.write().await;
            model.query_hover(fid)
        }
        None => None,
    };
    Ok(Json(PointerReport { event, hover }))
}
