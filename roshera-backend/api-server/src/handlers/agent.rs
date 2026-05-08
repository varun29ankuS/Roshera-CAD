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
    DatumSummary, DistanceReport, EdgeReport, FaceReport, ListPartsFilter, MassPropertiesReport,
    OrientedBBox, PartProximity, PartReport, PartSummary,
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
    State(state): State<AppState>,
    Query(q): Query<ListPartsQuery>,
) -> Json<Vec<PartSummary>> {
    let model = state.model.read().await;
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
    State(state): State<AppState>,
    Path(id): Path<SolidId>,
) -> Result<Json<PartReport>, StatusCode> {
    let mut model = state.model.write().await;
    model
        .query_part(id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/{id}/mass` — mass properties (volume, mass,
/// COG, inertia tensor, principal axes, radius of gyration).
///
/// Cache-warming on first call — takes a write lock because the kernel
/// populates per-entity caches during the divergence-theorem integral.
pub async fn part_mass_properties(
    State(state): State<AppState>,
    Path(id): Path<SolidId>,
) -> Result<Json<MassPropertiesReport>, StatusCode> {
    let mut model = state.model.write().await;
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
    Path(uuid): Path<uuid::Uuid>,
) -> Result<Json<MassPropertiesReport>, StatusCode> {
    let solid_id = state
        .get_local_id(&uuid)
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut model = state.model.write().await;
    model
        .mass_properties_for(solid_id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/{id}/obb` — oriented bounding box (axes
/// aligned to the part's principal moments of inertia).
pub async fn part_oriented_bbox(
    State(state): State<AppState>,
    Path(id): Path<SolidId>,
) -> Result<Json<OrientedBBox>, StatusCode> {
    let mut model = state.model.write().await;
    model
        .oriented_bbox_for(id)
        .map(Json)
        .ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/parts/distance/{a}/{b}` — bbox-center, AABB-gap,
/// overlap, and direction unit vector between two parts.
pub async fn part_distance(
    State(state): State<AppState>,
    Path((a, b)): Path<(SolidId, SolidId)>,
) -> Result<Json<DistanceReport>, StatusCode> {
    let model = state.model.read().await;
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
    State(state): State<AppState>,
    Path(id): Path<SolidId>,
    Json(req): Json<ReanchorPartRequest>,
) -> Result<Json<ReanchorPartResponse>, StatusCode> {
    use geometry_engine::readable::query::ReanchorError;

    let mut model = state.model.write().await;
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
pub async fn list_datums(State(state): State<AppState>) -> Json<Vec<DatumSummary>> {
    let model = state.model.read().await;
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
    State(state): State<AppState>,
    Path(id): Path<DatumId>,
    Query(q): Query<PartsNearDatumQuery>,
) -> Json<Vec<PartProximity>> {
    let model = state.model.read().await;
    Json(model.parts_near_datum(id, q.radius))
}

// ───────────────────── faces & edges ────────────────────────────────

/// `GET /api/agent/faces/{id}` — per-face report (surface type, area,
/// boundary edges, neighbouring faces, host solid).
pub async fn query_face(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<FaceReport>, StatusCode> {
    let mut model = state.model.write().await;
    model.query_face(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

/// `GET /api/agent/edges/{id}` — per-edge report (curve kind, length,
/// start/end vertex world coordinates).
pub async fn query_edge(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<EdgeReport>, StatusCode> {
    let mut model = state.model.write().await;
    model.query_edge(id).map(Json).ok_or(StatusCode::NOT_FOUND)
}
