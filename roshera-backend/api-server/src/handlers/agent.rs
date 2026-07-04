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
use geometry_engine::readable::claim::{
    verify_claim, CheckableClaim, ClaimBinding, ClaimVerdict, Measurement,
};
use geometry_engine::readable::{
    DatumDescriptor, DatumSummary, DistanceReport, EdgeReport, FaceReport, HoverReport,
    ListPartsFilter, MassPropertiesReport, OrientedBBox, PartProximity, PartReport, PartSummary,
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

/// One variable→measurement binding in a verify-claim request. Parts are
/// addressed by public UUID (the wire id); resolved to a kernel `SolidId`
/// before measuring. A `constant` carries a supplied non-geometric value.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VerifyMeasure {
    Volume {
        part: uuid::Uuid,
    },
    SurfaceArea {
        part: uuid::Uuid,
    },
    /// A single face's area (faces are addressed by kernel face id, not UUID —
    /// the agent gets them from render_part `ids` / get_face / select_face).
    FaceArea {
        face: u32,
    },
    /// A single edge's length (kernel edge id).
    EdgeLength {
        edge: u32,
    },
    Constant {
        value: f64,
    },
}

/// A `(var, measure)` pair: binds a variable name in the claim expression to a
/// measurement.
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyBinding {
    pub var: String,
    pub measure: VerifyMeasure,
}

/// Request body for `POST /api/agent/verify-claim`.
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyClaimRequest {
    /// Math expression over the binding variable names, e.g. `"a / v"`.
    pub expr: String,
    pub bindings: Vec<VerifyBinding>,
    pub expected: f64,
    #[serde(default)]
    pub tolerance: Option<f64>,
}

/// `POST /api/agent/verify-claim` — "the notebook that can't lie". Checks an
/// equation/numeric claim against the kernel's GROUND-TRUTH geometry: resolves
/// each part UUID to its `SolidId`, then runs the kernel verifier, returning the
/// three-state verdict (verified / false / refused). A part UUID that doesn't
/// resolve yields `refused` (a structured "couldn't measure that"), not a 404 —
/// the verifier never silently passes.
pub async fn verify_claim_handler(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(req): Json<VerifyClaimRequest>,
) -> Json<ClaimVerdict> {
    let mut bindings = Vec::with_capacity(req.bindings.len());
    let mut unresolved: Vec<String> = Vec::new();
    for b in req.bindings {
        let measure = match b.measure {
            VerifyMeasure::Constant { value } => Some(Measurement::Constant { value }),
            VerifyMeasure::Volume { part } => state
                .get_local_id(&part)
                .map(|solid| Measurement::Volume { solid }),
            VerifyMeasure::SurfaceArea { part } => state
                .get_local_id(&part)
                .map(|solid| Measurement::SurfaceArea { solid }),
            VerifyMeasure::FaceArea { face } => Some(Measurement::FaceArea { face }),
            VerifyMeasure::EdgeLength { edge } => Some(Measurement::EdgeLength { edge }),
        };
        match measure {
            Some(m) => bindings.push(ClaimBinding {
                var: b.var,
                measure: m,
            }),
            None => unresolved.push(b.var),
        }
    }

    // A binding whose part UUID didn't resolve cannot be measured — refuse
    // honestly rather than dropping it and evaluating an incomplete expression.
    if !unresolved.is_empty() {
        return Json(ClaimVerdict {
            verified: false,
            refused: true,
            computed: None,
            expected: req.expected,
            abs_error: None,
            tolerance_used: req
                .tolerance
                .unwrap_or_else(|| req.expected.abs().max(1.0) * 1e-6),
            resolved: Vec::new(),
            unresolved,
        });
    }

    let claim = CheckableClaim {
        expr: req.expr,
        bindings,
        expected: req.expected,
        tolerance: req.tolerance,
    };
    let mut model = model_handle.write().await;
    Json(verify_claim(&claim, &mut model))
}

/// `GET /api/agent/parts/{id}/profile` — the editable MERIDIAN a revolved part
/// was built from: the `(r, z)` half-plane points (returned as `[[r,z],...]`),
/// recovered from the part's retained construction geometry. This is the
/// scientist's editable profile — open it, change a radius, and re-revolve (the
/// #25 edit→regenerate loop). 404 when the part carries no retained profile (it
/// was not built by a parametric revolve).
pub async fn part_revolve_profile(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<SolidId>,
) -> Result<Json<Vec<[f64; 2]>>, StatusCode> {
    // The persistent profile store is replay-proof; prefer it, then fall back to
    // the kernel construction geometry.
    if let Some(p) = state.solid_profiles.get(&id) {
        return Ok(Json(p.clone()));
    }
    let model = model_handle.read().await;
    geometry_engine::operations::revolve::get_revolve_meridian(&model, id)
        .map(|rz| Json(rz.into_iter().map(|(r, z)| [r, z]).collect()))
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
    /// When `true`, overlay this part's labels as named callouts (the LABELLER
    /// eye) — a leader from each labelled entity's projected anchor to its name.
    /// Default `false`. Honoured in the `shaded` mode (the readable backdrop).
    pub labels: Option<bool>,
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
    use geometry_engine::render::{
        render_solid, render_solid_with_label_marks, CanonicalView, LabelMark, RenderMode,
        RenderOptions,
    };

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
    let opts = RenderOptions {
        width: size,
        height: size,
        view,
        mode,
        ..Default::default()
    };

    let frame = if q.labels.unwrap_or(false) {
        // Overlay path: needs a write lock (centroid anchors warm a cache) and
        // the label-aware renderer. Build a callout per label that has a world
        // anchor (name uppercased so the engineering 5×7 font renders it).
        let mut model = model_handle.write().await;
        // COLOR-CODED set-of-marks: each callout = "name — measurement.display",
        // drawn in the label's deterministic colour, with the target face tinted
        // the same colour so the picture itself maps mark → feature.
        let names: Vec<String> = model.list_labels().into_iter().map(|(n, ..)| n).collect();
        let mut marks: Vec<LabelMark> = Vec::new();
        for name in names {
            let Some(anchor) = model.label_anchor(&name) else {
                continue;
            };
            let color = geometry_engine::labels::label_color(&name);
            let measurement = model.label_measurement(&name);
            // The labelled face id (if it is a face label) so the render can tint
            // it; non-face labels still get a coloured callout.
            let target_face = model.resolve_label_face(&name).ok();
            let text = match &measurement {
                Some(m) => format!("{} - {}", name.to_uppercase(), m.display),
                None => name.to_uppercase(),
            };
            marks.push(LabelMark {
                anchor,
                text,
                color,
                target_face,
            });
        }
        render_solid_with_label_marks(&model, id as SolidId, &marks, &opts)
            .ok_or(StatusCode::NOT_FOUND)?
    } else {
        let model = model_handle.read().await;
        render_solid(&model, id as SolidId, &opts).ok_or(StatusCode::NOT_FOUND)?
    };
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

// ───────────────────── section / clip (EYE-2) ───────────────────────

/// Query for `GET /api/agent/parts/{id}/section` — a cutting plane (point +
/// normal). Defaults to the world XY plane through the origin.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct SectionQuery {
    pub px: Option<f64>,
    pub py: Option<f64>,
    pub pz: Option<f64>,
    pub nx: Option<f64>,
    pub ny: Option<f64>,
    pub nz: Option<f64>,
}

/// The section's in-plane camera transform (true-shape, looking along normal).
#[derive(Debug, Clone, Serialize)]
pub struct SectionCameraWire {
    pub right: Vec3Wire,
    pub up: Vec3Wire,
    pub scale: f64,
    pub ox: f64,
    pub oy: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub units: String,
    pub plane_origin: Vec3Wire,
    pub plane_normal: Vec3Wire,
    /// Measured cross-section area (mm²).
    pub section_area: f64,
    pub extent_u: f64,
    pub extent_v: f64,
    pub camera: SectionCameraWire,
}

/// `GET /api/agent/parts/{id}/section?px&py&pz&nx&ny&nz` — EYE-2.
///
/// Cuts the solid by the plane and returns a true-shape, dimensioned
/// cross-section render plus the measured area/extents and the in-plane camera
/// (so section points are recoverable from frame + query). Read lock only;
/// `404` if the plane misses the solid or the id is unknown.
pub async fn part_section(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<SectionQuery>,
) -> Result<Json<SectionResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::math::{Point3, Tolerance, Vector3};
    use geometry_engine::render::dimensioned::render_section;

    let origin = Point3::new(
        q.px.unwrap_or(0.0),
        q.py.unwrap_or(0.0),
        q.pz.unwrap_or(0.0),
    );
    let normal = Vector3::new(
        q.nx.unwrap_or(0.0),
        q.ny.unwrap_or(0.0),
        q.nz.unwrap_or(1.0),
    );

    let model = model_handle.read().await;
    let f = render_section(&model, id as SolidId, origin, normal, Tolerance::default())
        .ok_or(StatusCode::NOT_FOUND)?;
    let png = f.to_png().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(SectionResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: f.width,
        height: f.height,
        units: f.units.to_string(),
        plane_origin: f.plane_origin.into(),
        plane_normal: f.plane_normal.into(),
        section_area: f.section_area,
        extent_u: f.extent_u,
        extent_v: f.extent_v,
        camera: SectionCameraWire {
            right: f.right.into(),
            up: f.up.into(),
            scale: f.scale,
            ox: f.ox,
            oy: f.oy,
        },
    }))
}

// ───────────────────── axial profile (EYE-PROFILE) ──────────────────

/// Query for `GET /api/agent/parts/{id}/profile`. With no parameters the axis
/// of symmetry is auto-detected from the geometry. To override it, supply an
/// axis point (`ox,oy,oz`) AND direction (`ax,ay,az`); a partial override falls
/// back to auto-detection.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ProfileQuery {
    pub ox: Option<f64>,
    pub oy: Option<f64>,
    pub oz: Option<f64>,
    pub ax: Option<f64>,
    pub ay: Option<f64>,
    pub az: Option<f64>,
}

/// One measured feature dimension on the meridian.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileDimensionWire {
    pub kind: String,
    pub value: f64,
    pub unit: String,
    pub label: String,
    /// Axial station the dimension is taken at (diameters); `null` for spans.
    pub station: Option<f64>,
}

/// Wire shape for the dimensioned axial-profile drawing.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub units: String,
    /// Detected (or supplied) axis of symmetry.
    pub axis_origin: Vec3Wire,
    pub axis_dir: Vec3Wire,
    /// `true` when the section showed an inner wall (a hollow part).
    pub hollow: bool,
    /// Measured feature dimensions (overall length, Ø max/min/exit/base, wall
    /// thickness if hollow, dominant half-angle). The image is the placed
    /// table; this is authoritative.
    pub dimensions: Vec<ProfileDimensionWire>,
}

/// `GET /api/agent/parts/{id}/profile` — EYE-PROFILE, the engineering meridian.
///
/// Sections the solid by a plane CONTAINING its axis of symmetry and returns a
/// dimensioned axial-profile drawing: the meridian outline + chain-dash
/// centerline + feature dimensions (diameters, station heights, wall thickness,
/// half-angle), all MEASURED off the cut — never assumed. The axis is
/// auto-detected (Z fall-back) or supplied via query. Read lock only; `404` on
/// unknown id or when the part has no usable axial section.
pub async fn part_profile(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<ProfileQuery>,
) -> Result<Json<ProfileResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::math::{Point3, Tolerance, Vector3};
    use geometry_engine::render::profile::render_axial_profile;

    // A full axis override requires both an origin AND a direction; anything
    // less defers to auto-detection.
    let axis_override = match (q.ox, q.oy, q.oz, q.ax, q.ay, q.az) {
        (Some(ox), Some(oy), Some(oz), Some(ax), Some(ay), Some(az)) => {
            Some((Point3::new(ox, oy, oz), Vector3::new(ax, ay, az)))
        }
        _ => None,
    };

    let model = model_handle.read().await;
    let frame = render_axial_profile(&model, id as SolidId, axis_override, Tolerance::default())
        .ok_or(StatusCode::NOT_FOUND)?;
    let png = frame
        .to_png()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok(Json(ProfileResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: frame.width,
        height: frame.height,
        units: frame.units.to_string(),
        axis_origin: frame.axis_origin.into(),
        axis_dir: frame.axis_dir.into(),
        hollow: frame.hollow,
        dimensions: frame
            .dimensions
            .into_iter()
            .map(|d| ProfileDimensionWire {
                kind: d.kind,
                value: d.value,
                unit: d.unit,
                label: d.label,
                station: d.station,
            })
            .collect(),
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
    /// Tessellation quality of the rendered mesh: `coarse` | `medium` (default)
    /// | `fine`. Coarse is fast for quick orbits; fine resolves curved-surface
    /// silhouettes for inspection.
    pub quality: Option<String>,
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

/// `GET /api/agent/scene/orbit?az&el&mode&size` — the agent's eye on the WHOLE
/// SCENE. Composites every solid in the model into one frame from an arbitrary
/// azimuth/elevation (world Z up), auto-framed to the combined bounds. This is
/// what lets the agent drive the camera and SEE the full assembly (a car, a
/// mechanism) rather than one part at a time. Read lock only; `404` if the scene
/// is empty / tessellates to nothing, `400` on an unknown mode.
pub async fn scene_orbit(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Query(q): Query<OrbitQuery>,
) -> Result<Json<OrbitResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::math::Vector3;
    use geometry_engine::render::{render_solids_dir, CanonicalView, RenderMode, RenderOptions};
    use geometry_engine::tessellation::TessellationParams;

    let az = q.az.unwrap_or(45.0).to_radians();
    let el = q.el.unwrap_or(30.0).to_radians();
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
    let size = q.size.unwrap_or(640).clamp(64, 2048);

    let model = model_handle.read().await;
    let ids: Vec<SolidId> = model.solids.iter().map(|(id, _)| id).collect();
    if ids.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }
    // Per-solid colours from the registry (set via .../color); default light grey.
    let colors: Vec<[u8; 3]> = ids
        .iter()
        .map(|id| {
            state
                .solid_colors
                .get(id)
                .map(|c| *c)
                .unwrap_or([200, 200, 200])
        })
        .collect();
    let frame = render_solids_dir(
        &model,
        &ids,
        &colors,
        dir,
        up_hint,
        &RenderOptions {
            width: size,
            height: size,
            view: CanonicalView::Isometric, // ignored by the direction render
            mode,
            tessellation: match q.quality.as_deref() {
                Some("coarse") => TessellationParams::coarse(),
                Some("fine") => TessellationParams::fine(),
                _ => TessellationParams::default(),
            },
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

// ───────────────────── ground truth (PILLAR 1) ──────────────────────

/// The kernel's self-reported ground truth for a solid: provenance (what op made
/// Per-face display-tessellation defect — the agent's pointer to exactly which
/// face renders wrong, without rendering a pixel.
#[derive(Debug, Clone, Serialize)]
pub struct TessFaceDefectResponse {
    pub face_id: u64,
    pub triangles: usize,
    pub degenerate_triangles: usize,
    pub normal_agreement: f64,
    /// Winding-vs-TRUE-surface-normal agreement for this face — the decisive
    /// scribble signal (a bore whose stored normals are wrong-but-consistent
    /// drops here while `normal_agreement` stays high).
    pub analytic_normal_agreement: f64,
}

/// Display-tessellation quality — the render-mesh analogue of B-Rep soundness.
/// A watertight + manifold solid can still tessellate to a degenerate /
/// inverted-normal mesh (the inner-bore scribble); `clean == false` is that
/// defect, surfaced so the agent can SEE it without a render round-trip.
#[derive(Debug, Clone, Serialize)]
pub struct TessQualityResponse {
    pub clean: bool,
    pub triangles: usize,
    pub degenerate_triangles: usize,
    /// Fraction of facets whose winding normal agrees with their stored
    /// (analytic-intent) vertex normals. `1.0` = every facet shaded correctly.
    pub normal_agreement: f64,
    /// Fraction of facets whose winding normal agrees with the TRUE surface
    /// normal at the facet centroid — the ground-truth scribble check.
    pub analytic_normal_agreement: f64,
    pub inconsistent_facets: usize,
    /// Facets pointing into the wrong hemisphere of the true surface normal.
    pub off_surface_facets: usize,
    pub worst_face: Option<TessFaceDefectResponse>,
}

/// Per-face mesh-shape defect (the CAD mesh-quality rules).
#[derive(Debug, Clone, Serialize)]
pub struct MeshFaceQualityResponse {
    pub face_id: u64,
    pub worst_aspect_ratio: f64,
    pub min_angle_deg: f64,
    pub max_normal_deviation_deg: f64,
    pub boundary_crossing_facets: usize,
}

/// Mesh-quality verdict — the render mesh against the CAD tessellation rules
/// (boundary conformance, normal deviation, aspect ratio, min angle). Catches a
/// sliver "wing" / bridging facet that is watertight + correctly oriented.
#[derive(Debug, Clone, Serialize)]
pub struct MeshQualityResponse {
    pub clean: bool,
    pub triangles: usize,
    pub worst_aspect_ratio: f64,
    pub min_angle_deg: f64,
    pub max_normal_deviation_deg: f64,
    pub boundary_crossing_facets: usize,
    pub worst_face: Option<MeshFaceQualityResponse>,
}

/// it, designed vs bare primitive) + a COMPUTED validity certificate.
#[derive(Debug, Clone, Serialize)]
pub struct TruthResponse {
    pub solid_id: u32,
    /// Operation that created it, e.g. "nurbs_loft", "primitive:Box", "boolean".
    pub origin: String,
    /// A genuine designed feature (not a bare primitive stand-in / unrecorded).
    pub designed: bool,
    pub primitive: bool,
    pub inputs: Vec<u32>,
    pub brep_valid: bool,
    pub watertight: bool,
    pub manifold: bool,
    pub self_intersection_free: bool,
    pub euler_characteristic: i64,
    /// Cross-entity consistency verdict for the solid's linked construction
    /// geometry: "consistent" / "inconsistent" / "not_applicable".
    pub construction_consistent: String,
    /// D4 labels-consistency verdict: are all the part's labels still backed by
    /// a holding assertion? "consistent" / "inconsistent" / "not_applicable".
    /// An annotation flag — does NOT affect `sound`.
    pub labels_consistent: String,
    /// Display-tessellation quality: `false` ⇒ the render mesh is degenerate or
    /// has inverted normals (the inner-bore scribble) even if the B-Rep is sound.
    /// Factored into `sound`.
    pub tessellation_clean: bool,
    /// Full tessellation-quality breakdown incl. the worst defective face.
    pub tessellation: TessQualityResponse,
    /// Mesh-quality (CAD tessellation-rule) verdict: `false` ⇒ a facet violates a
    /// shape rule (a sliver "wing", a boundary-crossing/bridging facet, a far-off-
    /// surface normal) even if the mesh is watertight. Factored into `sound`.
    pub mesh_quality_clean: bool,
    /// Full mesh-quality breakdown incl. the worst face + which rule it fails.
    pub mesh_quality: MeshQualityResponse,
    /// Real, closed, manufacturable solid (brep_valid ∧ watertight ∧ manifold
    /// ∧ self-intersection-free ∧ construction-consistent ∧ tessellation-clean
    /// ∧ mesh-quality-clean).
    pub sound: bool,
    pub errors: Vec<String>,
    pub summary: String,
}

/// `GET /api/agent/parts/{id}/truth` — "what did you actually make, and is it
/// real?" answered by the KERNEL (provenance + computed certificate), never by
/// the agent. Read lock only; `404` on unknown id.
pub async fn part_truth(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<TruthResponse>, StatusCode> {
    // Write lock: `ground_truth` re-runs Pillar-3 selectors for the D4 labels
    // flag, which warm a per-face centroid cache (no geometry is mutated).
    let mut model = model_handle.write().await;
    let gt = model
        .ground_truth(id as SolidId)
        .ok_or(StatusCode::NOT_FOUND)?;
    let (origin, designed, primitive, inputs) = match &gt.provenance {
        Some(p) => (
            p.created_by.label(),
            p.created_by.is_designed(),
            p.created_by.is_primitive(),
            p.inputs.clone(),
        ),
        None => ("unrecorded".to_string(), false, false, Vec::new()),
    };
    let c = &gt.certificate;
    Ok(Json(TruthResponse {
        solid_id: id,
        origin,
        designed,
        primitive,
        inputs,
        brep_valid: c.brep_valid,
        watertight: c.watertight,
        manifold: c.manifold,
        self_intersection_free: c.self_intersection_free,
        euler_characteristic: c.euler_characteristic,
        construction_consistent: c.construction_consistent.label().to_string(),
        labels_consistent: c.labels_consistent.label().to_string(),
        tessellation_clean: c.tessellation.clean,
        tessellation: TessQualityResponse {
            clean: c.tessellation.clean,
            triangles: c.tessellation.triangles,
            degenerate_triangles: c.tessellation.degenerate_triangles,
            normal_agreement: c.tessellation.normal_agreement,
            analytic_normal_agreement: c.tessellation.analytic_normal_agreement,
            inconsistent_facets: c.tessellation.inconsistent_facets,
            off_surface_facets: c.tessellation.off_surface_facets,
            worst_face: c
                .tessellation
                .worst_face
                .as_ref()
                .map(|w| TessFaceDefectResponse {
                    face_id: w.face_id,
                    triangles: w.triangles,
                    degenerate_triangles: w.degenerate_triangles,
                    normal_agreement: w.normal_agreement,
                    analytic_normal_agreement: w.analytic_normal_agreement,
                }),
        },
        mesh_quality_clean: c.mesh_quality.clean,
        mesh_quality: MeshQualityResponse {
            clean: c.mesh_quality.clean,
            triangles: c.mesh_quality.triangles,
            worst_aspect_ratio: c.mesh_quality.worst_aspect_ratio,
            min_angle_deg: c.mesh_quality.min_angle_deg,
            max_normal_deviation_deg: c.mesh_quality.max_normal_deviation_deg,
            boundary_crossing_facets: c.mesh_quality.boundary_crossing_facets,
            worst_face: c
                .mesh_quality
                .worst_face
                .as_ref()
                .map(|w| MeshFaceQualityResponse {
                    face_id: w.face_id,
                    worst_aspect_ratio: w.worst_aspect_ratio,
                    min_angle_deg: w.min_angle_deg,
                    max_normal_deviation_deg: w.max_normal_deviation_deg,
                    boundary_crossing_facets: w.boundary_crossing_facets,
                }),
        },
        sound: c.is_sound(),
        errors: c.errors.clone(),
        summary: gt.summary(),
    }))
}

// ───────────────────── occupancy X-ray ──────────────────────────────

/// Query for `GET /api/agent/parts/{id}/occupancy?n=20` — the grid resolution.
/// Clamped to `[4, 48]`; larger grids are rejected because the ASCII slice-stack
/// and the full-grid SDF cost both blow up cubically. Defaults to 20.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct OccupancyQuery {
    pub n: Option<usize>,
}

/// Response for the occupancy X-ray. `dims` is `[x, y, z]` (all `n` for the
/// cubic grid). `slices` is the fixed ASCII slice-stack (`'#'`=inside,
/// `'.'`=outside; per-z header `z=k`, then `n` rows of `n` chars, rows by y,
/// cols by x).
#[derive(Debug, Clone, Serialize)]
pub struct OccupancyResponse {
    pub n: usize,
    pub bbox: OccupancyBBoxWire,
    pub dims: [usize; 3],
    pub fill_fraction: f64,
    pub slices: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct OccupancyBBoxWire {
    pub min: Vec3Wire,
    pub max: Vec3Wire,
}

/// `GET /api/agent/parts/{id}/occupancy?n=20` — the SDF X-ray.
///
/// Samples the part's EXACT signed-distance field into a coarse `n×n×n`
/// occupancy grid over its (margin-expanded) world bbox and returns it as an
/// ASCII slice-stack — the non-deceivable structural complement to the shaded
/// render and the certificate. Reveals internal cavities, wall thickness, gaps
/// and through-holes a render can occlude. Read lock only (`signed_distance` is
/// a read-only query); `n` clamped to `[4, 48]`; `404` on unknown id.
pub async fn part_occupancy(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<OccupancyQuery>,
) -> Result<Json<OccupancyResponse>, StatusCode> {
    use geometry_engine::queries::occupancy::{occupancy_grid, to_slice_stack};

    // Clamp to the supported range: below 4 the X-ray is too coarse to read,
    // above 48 the ASCII and full-grid SDF cost blow up cubically.
    let n = q.n.unwrap_or(20).clamp(4, 48);

    let model = model_handle.read().await;
    // Unknown id (or a solid with no boundary face) → 404 rather than an
    // all-empty grid, so the agent fails loudly on a bad reference.
    if model.solids.get(id as SolidId).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let grid = occupancy_grid(&model, id as SolidId, n, 0.1);
    let slices = to_slice_stack(&grid);

    Ok(Json(OccupancyResponse {
        n: grid.n,
        bbox: OccupancyBBoxWire {
            min: grid.bbox_min.into(),
            max: grid.bbox_max.into(),
        },
        dims: [grid.n, grid.n, grid.n],
        fill_fraction: grid.fill_fraction,
        slices,
    }))
}

/// Body for `POST /api/agent/parts/{id}/color` — RGB 0..255.
#[derive(Debug, Clone, Deserialize)]
pub struct ColorBody {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// `POST /api/agent/parts/{id}/color` — set a part's display colour, consumed by
/// the scene-eye (`/api/agent/scene/orbit`) so the agent sees a coloured
/// assembly. Registry-only (no geometry mutation); returns the stored colour.
pub async fn set_part_color(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(c): Json<ColorBody>,
) -> Json<serde_json::Value> {
    state.solid_colors.insert(id, [c.r, c.g, c.b]);
    // Push the colour to the live viewport: resolve the kernel solid id to its
    // public object UUID and broadcast an `ObjectColor` frame. Registry-only
    // when there's no UUID mapping (e.g. a solid never surfaced to the scene) —
    // the colour is still stored for the scene-eye, we just skip the broadcast.
    if let Some(uuid) = state.get_uuid(id) {
        crate::broadcast_object_color(&uuid.to_string(), [c.r, c.g, c.b]);
    }
    Json(serde_json::json!({
        "success": true,
        "part_id": id,
        "color": [c.r, c.g, c.b],
    }))
}

/// `GET /api/agent/parts/{id}/color` — read a part's display colour. The read
/// half of the colour registry: `color` is `[r,g,b]` if one was set, else `null`
/// (the viewport/scene-eye then uses its default grey). Lets the frontend (#12)
/// apply per-object colour without re-deriving it.
pub async fn get_part_color(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Json<serde_json::Value> {
    let color = state.solid_colors.get(&id).map(|c| *c);
    Json(serde_json::json!({ "part_id": id, "color": color }))
}

// ───────────────────── descriptive selection (PILLAR 3) ─────────────

/// `POST /api/agent/parts/{id}/select-face` — resolve a face by DESCRIPTION, or
/// REFUSE. Body: `{ "kind": "planar|cylindrical|spherical|conical|toroidal|nurbs|any",
/// "normal_dir": [x,y,z]?, "angle_tol_deg": 12?, "extremal":
/// "none|largest_area|smallest_area|most_along", "along": [x,y,z]? }`.
/// 200 → `{face_id, persistent_id}`; 404 → not found; 409 → ambiguous (with the
/// candidate face ids). The kernel never guesses — see queries::select.
pub async fn select_face(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::math::Vector3;
    use geometry_engine::queries::select::{
        resolve_face, Extremal, FaceQuery, SelectError, SurfaceKind,
    };

    let kind = match body.get("kind").and_then(|v| v.as_str()).unwrap_or("any") {
        "planar" | "plane" => SurfaceKind::Planar,
        "cylindrical" | "cylinder" => SurfaceKind::Cylindrical,
        "spherical" | "sphere" => SurfaceKind::Spherical,
        "conical" | "cone" => SurfaceKind::Conical,
        "toroidal" | "torus" => SurfaceKind::Toroidal,
        "nurbs" | "nurbssurface" => SurfaceKind::Nurbs,
        _ => SurfaceKind::Any,
    };
    let vec3 = |key: &str| -> Option<Vector3> {
        body.get(key).and_then(|v| v.as_array()).and_then(|a| {
            if a.len() == 3 {
                Some(Vector3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
            } else {
                None
            }
        })
    };
    let normal_dir = vec3("normal_dir");
    let along = vec3("along").or(normal_dir);
    let extremal = match body
        .get("extremal")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
    {
        "largest_area" | "largest" => Extremal::LargestArea,
        "smallest_area" | "smallest" => Extremal::SmallestArea,
        "most_along" | "topmost" | "furthest" => Extremal::MostAlong(along.unwrap_or(Vector3::Z)),
        _ => Extremal::None,
    };
    let mut q = FaceQuery::new(kind);
    q.normal_dir = normal_dir;
    q.extremal = extremal;
    if let Some(t) = body.get("angle_tol_deg").and_then(|v| v.as_f64()) {
        q.angle_tol_deg = t;
    }

    let mut model = model_handle.write().await;
    match resolve_face(&mut model, id as SolidId, &q) {
        Ok(fid) => {
            let pid = model.face_pid(fid).map(|p| p.0.to_string());
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "resolved": true,
                    "part_id": id,
                    "face_id": fid,
                    // Stable id that survives edits (if assigned), so the agent
                    // can re-reference this face after a parametric change.
                    "persistent_id": pid,
                })),
            )
        }
        Err(SelectError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "resolved": false, "error": "not_found",
                "message": "no face matches that description",
            })),
        ),
        Err(SelectError::Ambiguous(candidates)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "resolved": false, "error": "ambiguous",
                "message": "several faces match equally well — refine the description",
                "candidates": candidates,
            })),
        ),
    }
}

/// `POST /api/agent/parts/{id}/select-edge` — resolve an edge by DESCRIPTION, or
/// REFUSE. Body: `{ "curve_kind": "line|arc|circle|nurbs|any", "blend":
/// "any|filleted|chamfered|unblended", "direction": [x,y,z]?, "angle_tol_deg":
/// 12?, "extremal": "none|longest|shortest|most_along", "along": [x,y,z]? }`.
/// 200 → `{edge_id, persistent_id}`; 404 → not found; 409 → ambiguous (candidate
/// edge ids). Mirrors `select_face`; the kernel never guesses.
pub async fn select_edge(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::math::Vector3;
    use geometry_engine::queries::select::{
        resolve_edge, BlendFilter, CurveKind, EdgeExtremal, EdgeQuery, SelectError,
    };

    let curve_kind = match body
        .get("curve_kind")
        .and_then(|v| v.as_str())
        .unwrap_or("any")
    {
        "line" => CurveKind::Line,
        "arc" => CurveKind::Arc,
        "circle" => CurveKind::Circle,
        "nurbs" => CurveKind::Nurbs,
        _ => CurveKind::Any,
    };
    let blend = match body.get("blend").and_then(|v| v.as_str()).unwrap_or("any") {
        "filleted" | "fillet" => BlendFilter::Filleted,
        "chamfered" | "chamfer" => BlendFilter::Chamfered,
        "unblended" | "none" => BlendFilter::Unblended,
        _ => BlendFilter::Any,
    };
    let vec3 = |key: &str| -> Option<Vector3> {
        body.get(key).and_then(|v| v.as_array()).and_then(|a| {
            if a.len() == 3 {
                Some(Vector3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
            } else {
                None
            }
        })
    };
    let direction = vec3("direction");
    let along = vec3("along").or(direction);
    let extremal = match body
        .get("extremal")
        .and_then(|v| v.as_str())
        .unwrap_or("none")
    {
        "longest" => EdgeExtremal::Longest,
        "shortest" => EdgeExtremal::Shortest,
        "most_along" | "furthest" => EdgeExtremal::MostAlong(along.unwrap_or(Vector3::Z)),
        _ => EdgeExtremal::None,
    };
    let mut q = EdgeQuery::new(curve_kind);
    q.blend = blend;
    q.direction = direction;
    q.extremal = extremal;
    if let Some(t) = body.get("angle_tol_deg").and_then(|v| v.as_f64()) {
        q.angle_tol_deg = t;
    }

    let mut model = model_handle.write().await;
    match resolve_edge(&mut model, id as SolidId, &q) {
        Ok(eid) => {
            let pid = model.edge_pid(eid).map(|p| p.0.to_string());
            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "resolved": true, "part_id": id, "edge_id": eid, "persistent_id": pid,
                })),
            )
        }
        Err(SelectError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "resolved": false, "error": "not_found",
                "message": "no edge matches that description",
            })),
        ),
        Err(SelectError::Ambiguous(candidates)) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "resolved": false, "error": "ambiguous",
                "message": "several edges match equally well — refine the description",
                "candidates": candidates,
            })),
        ),
    }
}

// ───────────────────── labels (the LABELLER) ────────────────────────

/// Map a kernel [`geometry_engine::labels::LabelError`] onto an HTTP verdict +
/// machine-readable body. `NotFound`/`Dangling` → 404, `EmptyName` → 400. The
/// kernel never guesses; the wire surface mirrors that refusal.
fn label_error_response(
    e: geometry_engine::labels::LabelError,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::labels::LabelError;
    match e {
        LabelError::EmptyName => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "resolved": false, "error": "empty_name",
                "message": "a label name must be non-empty",
            })),
        ),
        LabelError::NotFound => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "resolved": false, "error": "not_found",
                "message": "no label of that kind with that name on this part",
            })),
        ),
        LabelError::Dangling => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "resolved": false, "error": "dangling",
                "message": "the entity this label named no longer exists",
            })),
        ),
        LabelError::MissingAssertion => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "resolved": false, "error": "missing_assertion",
                "message": "an entity label must carry an assertion (a selector or fingerprint) — no bare labels",
            })),
        ),
        LabelError::NameInUse => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "resolved": false, "error": "name_in_use",
                "message": "another label already has that name",
            })),
        ),
    }
}

/// `POST /api/agent/parts/{id}/labels` — pin a human-readable NAME to a
/// topological entity or a cross-section. The shared vocabulary between agent
/// and user. Body (JSON):
/// * `{ "name": "throat", "kind": "vertex|edge|face", "entity_id": 12,
///     "description"?: "..." }` — attach BY ID, or
/// * `{ "name": "throat", "kind": "face", "selector": { ...select-face body... },
///     "description"?: "..." }` — attach BY DESCRIPTION (Pillar 3): the kernel
///     resolves the selector (or REFUSES on ambiguity/not-found) and labels the
///     result, or
/// * `{ "name": "midspan", "kind": "section", "origin": [x,y,z],
///     "normal": [x,y,z], "description"?: "..." }` — name a cutting plane.
///
/// 200 → `{ created|replaced, name, kind, entity_id?|plane? }`; 400 invalid
/// name/kind/body; 404 selector not-found; 409 selector ambiguous.
pub async fn create_label(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(body): Json<serde_json::Value>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::labels::{AttachOutcome, LabelKind};
    use geometry_engine::math::Vector3;

    let name = match body.get("name").and_then(|v| v.as_str()) {
        Some(n) => n.to_string(),
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "missing_name"})),
            )
        }
    };
    let description = body
        .get("description")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let kind_str = body.get("kind").and_then(|v| v.as_str()).unwrap_or("");
    let vec3 = |key: &str| -> Option<Vector3> {
        body.get(key).and_then(|v| v.as_array()).and_then(|a| {
            if a.len() == 3 {
                Some(Vector3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
            } else {
                None
            }
        })
    };

    let mut model = model_handle.write().await;

    // Section label: a named plane (not a topological entity).
    if kind_str.eq_ignore_ascii_case("section") {
        let (origin, normal) = match (vec3("origin"), vec3("normal")) {
            (Some(o), Some(n)) => (o, n),
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "error": "section_requires_origin_and_normal",
                    })),
                )
            }
        };
        let origin_pt = geometry_engine::math::Point3::new(origin.x, origin.y, origin.z);
        return match model.label_section(&name, origin_pt, normal, description) {
            Ok(outcome) => (
                StatusCode::OK,
                Json(serde_json::json!({
                    "success": true,
                    "created": outcome == AttachOutcome::Created,
                    "replaced": outcome == AttachOutcome::Replaced,
                    "part_id": id, "name": name, "kind": "section",
                    "plane": { "origin": [origin.x, origin.y, origin.z],
                               "normal": [normal.x, normal.y, normal.z] },
                })),
            ),
            Err(e) => label_error_response(e),
        };
    }

    // Entity label: vertex / edge / face, by id or by selector.
    let kind = match LabelKind::from_tag(kind_str) {
        Some(k) => k,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({
                    "error": "unknown_kind",
                    "message": "kind must be vertex|edge|face|section",
                })),
            )
        }
    };

    // Resolve the target entity id: explicit `entity_id`, or by `selector`.
    // D4: when attached BY SELECTOR, the selector IS the assertion (re-run on
    // every resolve); BY ID, the kernel captures the entity's fingerprint.
    let selector_spec: Option<geometry_engine::labels::SelectorSpec>;
    let entity_id: u32 = if let Some(eid) = body.get("entity_id").and_then(|v| v.as_u64()) {
        selector_spec = None;
        eid as u32
    } else if let Some(sel) = body.get("selector").filter(|v| !v.is_null()) {
        match resolve_selector(&mut model, id as SolidId, kind, sel) {
            Ok((eid, spec)) => {
                selector_spec = Some(spec);
                eid
            }
            Err(resp) => return resp,
        }
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "missing_target",
                "message": "provide entity_id or selector",
            })),
        );
    };

    let attach = match (kind, selector_spec) {
        (LabelKind::Face, Some(spec)) => model.label_face_with_assertion(
            entity_id,
            &name,
            geometry_engine::labels::LabelAssertion::Selector(spec),
            description,
        ),
        (LabelKind::Edge, Some(spec)) => model.label_edge_with_assertion(
            entity_id,
            &name,
            geometry_engine::labels::LabelAssertion::Selector(spec),
            description,
        ),
        (LabelKind::Face, None) => model.label_face(entity_id, &name, description),
        (LabelKind::Edge, None) => model.label_edge(entity_id, &name, description),
        (LabelKind::Vertex, _) => model.label_vertex(entity_id, &name, description),
    };
    match attach {
        Ok(outcome) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "success": true,
                "created": outcome == AttachOutcome::Created,
                "replaced": outcome == AttachOutcome::Replaced,
                "part_id": id, "name": name, "kind": kind.tag(),
                "entity_id": entity_id,
            })),
        ),
        Err(e) => label_error_response(e),
    }
}

/// Resolve a `selector` JSON body (the same shape as `select-face` /
/// `select-edge`) to a single entity id of the requested `kind`, or return the
/// refusal response. Only `face`/`edge` selectors are supported (a vertex has
/// no descriptive selector yet); a `vertex` kind with a selector is rejected.
fn resolve_selector(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    solid: SolidId,
    kind: geometry_engine::labels::LabelKind,
    sel: &serde_json::Value,
) -> Result<(u32, geometry_engine::labels::SelectorSpec), (StatusCode, Json<serde_json::Value>)> {
    use geometry_engine::labels::{EdgeSelectorSpec, FaceSelectorSpec, LabelKind, SelectorSpec};
    use geometry_engine::math::Vector3;
    use geometry_engine::queries::select::{
        resolve_edge, resolve_face, BlendFilter, CurveKind, EdgeExtremal, EdgeQuery, Extremal,
        FaceQuery, SelectError, SurfaceKind,
    };

    let vec3 = |key: &str| -> Option<Vector3> {
        sel.get(key).and_then(|v| v.as_array()).and_then(|a| {
            if a.len() == 3 {
                Some(Vector3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
            } else {
                None
            }
        })
    };
    let refuse = |e: SelectError| -> (StatusCode, Json<serde_json::Value>) {
        match e {
            SelectError::NotFound => (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "resolved": false, "error": "selector_not_found",
                    "message": "no entity matches that description",
                })),
            ),
            SelectError::Ambiguous(c) => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "resolved": false, "error": "selector_ambiguous",
                    "message": "several entities match — refine the description",
                    "candidates": c,
                })),
            ),
        }
    };

    let v3arr = |v: Option<Vector3>| v.map(|d| [d.x, d.y, d.z]);

    match kind {
        LabelKind::Face => {
            let surf_tag = sel
                .get("kind")
                .and_then(|v| v.as_str())
                .unwrap_or("any")
                .to_string();
            let surf = match surf_tag.as_str() {
                "planar" | "plane" => SurfaceKind::Planar,
                "cylindrical" | "cylinder" => SurfaceKind::Cylindrical,
                "spherical" | "sphere" => SurfaceKind::Spherical,
                "conical" | "cone" => SurfaceKind::Conical,
                "toroidal" | "torus" => SurfaceKind::Toroidal,
                "nurbs" | "nurbssurface" => SurfaceKind::Nurbs,
                _ => SurfaceKind::Any,
            };
            let normal_dir = vec3("normal_dir");
            let along = vec3("along").or(normal_dir);
            let extremal_tag = sel
                .get("extremal")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string();
            // Geometry-aware extremals (the throat / exit recognizers) carry the
            // symmetry axis in the selector body so a proposal confirms verbatim.
            let axis_origin = vec3("axis_origin");
            let axis_dir = vec3("axis_dir");
            let spec_axis = match (axis_origin, axis_dir) {
                (Some(o), Some(d)) => Some(geometry_engine::queries::select::Axis {
                    origin: o,
                    direction: d,
                }),
                _ => None,
            };
            let extremal = match extremal_tag.as_str() {
                "largest_area" | "largest" => Extremal::LargestArea,
                "smallest_area" | "smallest" => Extremal::SmallestArea,
                "most_along" | "topmost" | "furthest" => {
                    Extremal::MostAlong(along.unwrap_or(Vector3::Z))
                }
                "min_radius_station" | "min_radius" => match spec_axis {
                    Some(a) => Extremal::MinRadiusStation(a),
                    None => Extremal::None,
                },
                "axial_extremal_cap" | "axial_cap" => match spec_axis {
                    Some(a) => Extremal::AxialExtremalCap(a),
                    None => Extremal::None,
                },
                _ => Extremal::None,
            };
            let mut q = FaceQuery::new(surf);
            q.normal_dir = normal_dir;
            q.extremal = extremal;
            if let Some(t) = sel.get("angle_tol_deg").and_then(|v| v.as_f64()) {
                q.angle_tol_deg = t;
            }
            let spec = SelectorSpec::Face(FaceSelectorSpec {
                surface: surf_tag,
                normal_dir: v3arr(normal_dir),
                angle_tol_deg: q.angle_tol_deg,
                extremal: extremal_tag,
                along: v3arr(along),
                axis_origin: v3arr(axis_origin),
                axis_dir: v3arr(axis_dir),
            });
            resolve_face(model, solid, &q)
                .map(|fid| (fid, spec))
                .map_err(refuse)
        }
        LabelKind::Edge => {
            let curve_tag = sel
                .get("curve_kind")
                .and_then(|v| v.as_str())
                .unwrap_or("any")
                .to_string();
            let curve_kind = match curve_tag.as_str() {
                "line" => CurveKind::Line,
                "arc" => CurveKind::Arc,
                "circle" => CurveKind::Circle,
                "nurbs" => CurveKind::Nurbs,
                _ => CurveKind::Any,
            };
            let blend_tag = sel
                .get("blend")
                .and_then(|v| v.as_str())
                .unwrap_or("any")
                .to_string();
            let blend = match blend_tag.as_str() {
                "filleted" | "fillet" => BlendFilter::Filleted,
                "chamfered" | "chamfer" => BlendFilter::Chamfered,
                "unblended" | "none" => BlendFilter::Unblended,
                _ => BlendFilter::Any,
            };
            let direction = vec3("direction");
            let along = vec3("along").or(direction);
            let extremal_tag = sel
                .get("extremal")
                .and_then(|v| v.as_str())
                .unwrap_or("none")
                .to_string();
            let extremal = match extremal_tag.as_str() {
                "longest" => EdgeExtremal::Longest,
                "shortest" => EdgeExtremal::Shortest,
                "most_along" | "furthest" => EdgeExtremal::MostAlong(along.unwrap_or(Vector3::Z)),
                _ => EdgeExtremal::None,
            };
            let mut q = EdgeQuery::new(curve_kind);
            q.blend = blend;
            q.direction = direction;
            q.extremal = extremal;
            if let Some(t) = sel.get("angle_tol_deg").and_then(|v| v.as_f64()) {
                q.angle_tol_deg = t;
            }
            let spec = SelectorSpec::Edge(EdgeSelectorSpec {
                curve: curve_tag,
                blend: blend_tag,
                direction: v3arr(direction),
                angle_tol_deg: q.angle_tol_deg,
                extremal: extremal_tag,
                along: v3arr(along),
            });
            resolve_edge(model, solid, &q)
                .map(|eid| (eid, spec))
                .map_err(refuse)
        }
        LabelKind::Vertex => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "no_vertex_selector",
                "message": "a vertex label must be attached by entity_id (no descriptive selector)",
            })),
        )),
    }
}

/// `GET /api/agent/parts/{id}/labels` — list every label on this part: its
/// name, kind, optional world anchor (the callout point), and description. In
/// name order. Read path warms the centroid cache, so it takes a write lock.
pub async fn list_labels(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Json<serde_json::Value> {
    use geometry_engine::labels::AssertionStatus;
    let mut model = model_handle.write().await;
    // Snapshot (name, kind, anchor, description) first — list_labels warms the
    // centroid cache — then enrich each with measurement / colour / conformance /
    // stale. The per-label fields are ADDITIVE: existing consumers that read only
    // name / kind / anchor / description keep working unchanged.
    let base: Vec<(
        String,
        &'static str,
        Option<geometry_engine::math::Point3>,
        Option<String>,
    )> = model.list_labels();
    let labels: Vec<serde_json::Value> = base
        .into_iter()
        .map(|(name, kind, anchor, description)| {
            let color = geometry_engine::labels::label_color_hex(&name);
            let measurement = model.label_measurement(&name).map(|m| {
                serde_json::json!({
                    "value": m.value,
                    "unit": m.unit,
                    "kind": m.kind.tag(),
                    "display": m.display,
                })
            });
            let conformance = model.label_conformance(&name);
            // A section label is its own claim and always holds; an entity label
            // is stale when its assertion no longer re-verifies.
            let stale = model.verify_label_assertion_mut(&name) == AssertionStatus::Stale;
            serde_json::json!({
                "name": name,
                "kind": kind,
                "anchor": anchor.map(|p| [p.x, p.y, p.z]),
                "color": color,
                "measurement": measurement,
                "conformance": conformance,
                "stale": stale,
                "description": description,
            })
        })
        .collect();
    Json(serde_json::json!({ "part_id": id, "labels": labels }))
}

/// `GET /api/agent/parts/{id}/labels/{name}/resolve` — resolve a NAME back to
/// the live entity (or section plane) it pins. 200 → the resolved target; 404
/// → not-found / dangling; the kernel never guesses.
pub async fn resolve_label(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((id, name)): Path<(u32, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::labels::{AssertionStatus, LabelError, LabelKind, LabelTarget};
    // Write lock: the `_checked` resolve RE-RUNS the label's assertion (D4),
    // which warms the selector centroid cache. No geometry is mutated.
    let mut model = model_handle.write().await;
    let (target, description) = match model.label(&name) {
        Some(l) => (l.target.clone(), l.description.clone()),
        None => return label_error_response(LabelError::NotFound),
    };
    match target {
        LabelTarget::Section(p) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "resolved": true, "part_id": id, "name": name, "kind": "section",
                "plane": { "origin": [p.origin.x, p.origin.y, p.origin.z],
                           "normal": [p.normal.x, p.normal.y, p.normal.z] },
                "stale": false,
                "description": description,
            })),
        ),
        LabelTarget::Entity { kind, .. } => {
            // Resolve to a live id AND re-verify the assertion. A `Stale` verdict
            // is surfaced HONESTLY (`stale:true`) — the id is the named entity's
            // last-known live id, but the claim that justified the name no longer
            // holds, so the caller must not blindly trust it.
            let resolved = match kind {
                LabelKind::Face => model
                    .resolve_label_face_checked(&name)
                    .map(|(fid, st)| ("face", fid, st)),
                LabelKind::Edge => model
                    .resolve_label_edge_checked(&name)
                    .map(|(eid, st)| ("edge", eid, st)),
                LabelKind::Vertex => model
                    .resolve_label_vertex(&name)
                    .map(|vid| ("vertex", vid, model.verify_label_assertion(&name))),
            };
            match resolved {
                Ok((tag, eid, status)) => {
                    let stale = status == AssertionStatus::Stale;
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "resolved": true, "part_id": id, "name": name,
                            "kind": tag, "entity_id": eid,
                            "stale": stale,
                            "description": description,
                        })),
                    )
                }
                Err(e) => label_error_response(e),
            }
        }
    }
}

/// `DELETE /api/agent/parts/{id}/labels/{name}` — REMOVE a label by name. 200
/// when it existed and was removed; 404 when there was no such name (the kernel
/// reports honestly rather than pretending). Fixes the "a mislabel can't be
/// removed" gap.
pub async fn delete_label(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((id, name)): Path<(u32, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    let mut model = model_handle.write().await;
    if model.delete_label(&name) {
        (
            StatusCode::OK,
            Json(serde_json::json!({ "deleted": true, "part_id": id, "name": name })),
        )
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "deleted": false, "part_id": id, "name": name,
                "error": "not_found", "message": "no label of that name on this part",
            })),
        )
    }
}

/// Body for `PATCH /api/agent/parts/{id}/labels/{name}` — the new name.
#[derive(serde::Deserialize)]
pub struct RenameLabelRequest {
    pub new_name: String,
}

/// `PATCH /api/agent/parts/{id}/labels/{name}` — RENAME a label, preserving its
/// binding (target + assertion + description). 200 on success; 404 when `name`
/// is unknown; 409 when `new_name` is already taken by a different label (refuse,
/// never clobber); 400 when `new_name` is empty.
pub async fn rename_label(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((id, name)): Path<(u32, String)>,
    Json(body): Json<RenameLabelRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::labels::LabelError;
    let mut model = model_handle.write().await;
    match model.rename_label(&name, &body.new_name) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "renamed": true, "part_id": id,
                "old_name": name, "new_name": body.new_name.trim(),
            })),
        ),
        Err(LabelError::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "renamed": false, "part_id": id, "old_name": name,
                "error": "not_found", "message": "no label of that name on this part",
            })),
        ),
        Err(LabelError::NameInUse) => (
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "renamed": false, "part_id": id, "old_name": name, "new_name": body.new_name,
                "error": "name_in_use", "message": "another label already has that name; remove it first",
            })),
        ),
        Err(LabelError::EmptyName) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "renamed": false, "part_id": id, "old_name": name,
                "error": "empty_name", "message": "the new label name must be non-empty",
            })),
        ),
        Err(e) => label_error_response(e),
    }
}

/// `GET /api/agent/parts/{id}/propose-labels` — D3 AUTO-PROPOSE. The kernel
/// recognizes features (throat / exit / chamber / fillet …) and SUGGESTS a name
/// plus the ASSERTION that pins it — it does NOT apply them. Confirming a
/// proposal = `POST .../labels` with `name` + a `selector` equal to the
/// proposal's assertion (the user owns the name, the kernel owns the claim).
/// Write lock (the recognizers run Pillar-3 selectors that warm the centroid
/// cache); `404` on unknown id.
pub async fn propose_labels(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::labels::{LabelAssertion, SelectorSpec};
    let mut model = model_handle.write().await;
    if model.solids.get(id as SolidId).is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "unknown_part", "part_id": id })),
        );
    }
    let proposals = model.propose_labels(id as SolidId);
    // Render each proposal's assertion as the exact `selector` body the agent
    // would POST to confirm it — so confirming is a copy-paste, not a re-derive.
    let selector_json = |spec: &SelectorSpec| -> serde_json::Value {
        match spec {
            SelectorSpec::Face(s) => {
                let mut o = serde_json::Map::new();
                o.insert("kind".into(), serde_json::json!(s.surface));
                if let Some(n) = s.normal_dir {
                    o.insert("normal_dir".into(), serde_json::json!(n));
                }
                o.insert("extremal".into(), serde_json::json!(s.extremal));
                if let Some(a) = s.along {
                    o.insert("along".into(), serde_json::json!(a));
                }
                if let Some(ao) = s.axis_origin {
                    o.insert("axis_origin".into(), serde_json::json!(ao));
                }
                if let Some(ad) = s.axis_dir {
                    o.insert("axis_dir".into(), serde_json::json!(ad));
                }
                o.insert("angle_tol_deg".into(), serde_json::json!(s.angle_tol_deg));
                serde_json::Value::Object(o)
            }
            SelectorSpec::Edge(s) => {
                let mut o = serde_json::Map::new();
                o.insert("curve_kind".into(), serde_json::json!(s.curve));
                o.insert("blend".into(), serde_json::json!(s.blend));
                if let Some(d) = s.direction {
                    o.insert("direction".into(), serde_json::json!(d));
                }
                o.insert("extremal".into(), serde_json::json!(s.extremal));
                if let Some(a) = s.along {
                    o.insert("along".into(), serde_json::json!(a));
                }
                o.insert("angle_tol_deg".into(), serde_json::json!(s.angle_tol_deg));
                serde_json::Value::Object(o)
            }
        }
    };
    let body: Vec<serde_json::Value> = proposals
        .into_iter()
        .map(|p| {
            let selector = match &p.assertion {
                LabelAssertion::Selector(spec) => Some(selector_json(spec)),
                LabelAssertion::Fingerprint(_) => None,
            };
            serde_json::json!({
                "suggested_name": p.suggested_name,
                "kind": p.kind,
                "confidence": p.confidence,
                "rationale": p.rationale,
                "selector": selector,
            })
        })
        .collect();
    (
        StatusCode::OK,
        Json(serde_json::json!({ "part_id": id, "proposals": body })),
    )
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

// ───────────────────── perception (feedback-as-default) ─────────────

/// A part's self-reported soundness — watertight + valid + dims — queryable for
/// ANY existing solid, not just at mutation time. Feedback-as-default: the agent
/// (and the panel) can read current truth on demand without re-running the op.
#[derive(Debug, Clone, Serialize)]
pub struct PartPerception {
    pub solid_id: u32,
    /// AUTHORITATIVE verdict: the exact B-Rep validity (mesh-independent). This —
    /// not `watertight` — is the sound answer to "is this a real solid?".
    pub sound: bool,
    /// Human/agent-readable one-liner derived from `sound` + the mesh check.
    pub verdict: String,
    /// Export-mesh watertightness (display/STL quality) — a valid solid can show
    /// `false` here from tessellation T-junctions without being broken.
    pub watertight: bool,
    pub open_edges: usize,
    pub nonmanifold_edges: usize,
    pub valid: bool,
    /// [L, W, H] world extents, or null if degenerate.
    pub dims: Option<[f64; 3]>,
}

/// Query for `GET /api/agent/parts/{id}/perception`.
///
/// DEFAULT: the FULL kernel certificate (`certify_solid` — coarse internal
/// tessellation, memoized, O(n) spatial hash for self-intersection).
/// Callers opt OUT via `?fast=1` to get only the lightweight B-Rep + mesh-count
/// block (~5 ms, read lock only). `?full=1` is accepted as a no-op confirmation
/// (full cert is the default) for backward compatibility.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PerceptionQuery {
    #[serde(default)]
    pub fast: Option<String>,
    // NOTE: `?full=1` is a backward-compat no-op alias — full cert is now the
    // default. Axum's Query extractor ignores unknown params, so `?full=1`
    // requests still deserialize and route to the default full-cert path; no
    // field is needed for it.
}

impl PerceptionQuery {
    /// True iff the caller explicitly opted OUT of the full certificate via
    /// `?fast=1` / `?fast=true`.
    fn wants_fast(&self) -> bool {
        self.fast
            .as_deref()
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
}

/// `GET /api/agent/parts/{id}/perception` — the part's validity verdict.
///
/// DEFAULT: the FULL kernel certificate (`sound` = `certify_solid().is_sound()`,
/// every dimension: self-intersection, construction-consistency, labels,
/// tessellation- and mesh-quality). `certify_solid` uses a COARSE internal
/// tessellation (manifold @ chord 0.1, self-intersection @ chord 0.5) and is
/// MEMOIZED per solid (repeat calls are cache hits). Write lock — `certify_solid`
/// warms a per-face centroid cache; geometry is never mutated.
///
/// `?fast=1` OPTS OUT to the lightweight block (B-Rep validity + coarse mesh
/// counts + L×W×H, ~5 ms, read lock only). `?full=1` is a no-op alias that
/// continues to return the full cert (for backward compatibility). `404` on
/// unknown id / empty tessellation.
pub async fn part_perception(
    State(state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Query(q): Query<PerceptionQuery>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    use geometry_engine::harness::watertight::manifold_report;
    use geometry_engine::math::Tolerance;
    use geometry_engine::primitives::validation::{validate_solid_scoped, ValidationLevel};

    let sid = id as SolidId;

    if q.wants_fast() {
        // OPT-OUT (`?fast=1`): lightweight (cheap, read lock): B-Rep validity + coarse mesh counts + dims.
        let model = model_handle.read().await;
        if model.solids.get(sid).is_none() {
            return Err(StatusCode::NOT_FOUND);
        }
        let report = manifold_report(&model, sid, 0.5, 1e-6).ok_or(StatusCode::NOT_FOUND)?;
        let valid =
            validate_solid_scoped(&model, sid, Tolerance::default(), ValidationLevel::Standard)
                .is_valid;
        let dims = model.solid_world_bbox(sid).map(|b| {
            let s = b.size();
            [s.x, s.y, s.z]
        });
        let mesh_watertight = report.boundary_edges == 0 && report.nonmanifold_edges == 0;
        let verdict = if !valid {
            "BROKEN — B-Rep invalid (a real topological defect)".to_string()
        } else if mesh_watertight {
            "OK — valid closed solid; export mesh watertight".to_string()
        } else {
            "OK — valid B-Rep; export mesh has tessellation artifacts only (not a defect)"
                .to_string()
        };
        // Reconcile: always pending on the fast path — no write lock means no
        // `calculate_solid_volume`, so the write-path fingerprint cannot be
        // reproduced without upgrading the lock. Use the default (full) path to
        // get the reconcile report.
        let mut perception_val = serde_json::to_value(PartPerception {
            solid_id: id,
            sound: valid,
            verdict,
            watertight: mesh_watertight,
            open_edges: report.boundary_edges,
            nonmanifold_edges: report.nonmanifold_edges,
            valid,
            dims,
        })
        .unwrap_or_else(|_| serde_json::json!({}));
        if let serde_json::Value::Object(ref mut map) = perception_val {
            map.insert(
                "reconcile".to_string(),
                serde_json::json!({ "status": "pending" }),
            );
        }
        return Ok(Json(perception_val));
    }

    // DEFAULT (and `?full=1` no-op alias): the FULL certificate. Write lock —
    // `certify_solid` warms a per-face centroid cache; geometry is never mutated.
    let mut model = model_handle.write().await;
    if model.solids.get(sid).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let report = manifold_report(&model, sid, 0.5, 1e-6).ok_or(StatusCode::NOT_FOUND)?;
    let valid = validate_solid_scoped(&model, sid, Tolerance::default(), ValidationLevel::Standard)
        .is_valid;
    let dims = model.solid_world_bbox(sid).map(|b| {
        let s = b.size();
        [s.x, s.y, s.z]
    });
    let cert = model.certify_solid(sid);
    let sound = cert.is_sound();
    let verdict = if sound {
        "SOUND — full kernel certificate clean (closed, manifold, self-intersection-free, mesh-quality-clean)".to_string()
    } else {
        "UNSOUND — full kernel certificate flags a defect (see cert)".to_string()
    };
    // Reconcile cache lookup — mirrors the write path (certified_response in
    // main.rs): same four inputs, same `perception_fingerprint` function.
    // The write lock is already held so `calculate_solid_volume` (which warms
    // the mass-props cache) is safe here.
    let face_count = model.solid_outer_face_count(sid).unwrap_or(0) as u64;
    let volume = model.calculate_solid_volume(sid).unwrap_or(0.0);
    let fp = crate::perception_fingerprint(id, valid, face_count, volume);
    let reconcile_json = match state.reconcile_cache.get(&(id, fp)) {
        Some(rep) => serde_json::to_value(rep.value().as_ref())
            .unwrap_or_else(|_| serde_json::json!({ "status": "pending" })),
        None => serde_json::json!({ "status": "pending" }),
    };
    Ok(Json(serde_json::json!({
        "solid_id":          id,
        "sound":             sound,
        "verdict":           verdict,
        // B-Rep + export-mesh facts (kept for backward compatibility).
        "valid":             valid,
        "watertight":        cert.watertight,
        "open_edges":        report.boundary_edges,
        "nonmanifold_edges": report.nonmanifold_edges,
        "dims":              dims,
        // The full kernel certificate — every soundness dimension.
        "cert":              crate::certificate_json(&cert),
        // Advisory dual-eye reconcile report, or {"status":"pending"} when the
        // async worker has not yet completed a report for the current solid state.
        "reconcile":         reconcile_json,
    })))
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

/// One row of the analytic dimension table — recoverable (3D anchor + the face
/// ids it spans), read off analytic surfaces / exact curves, never the mesh.
#[derive(Debug, Clone, Serialize)]
pub struct DimensionWire {
    pub id: String,
    pub kind: String,
    pub value: f64,
    pub unit: String,
    pub label: String,
    pub entities: Vec<u32>,
    pub anchor: [f64; 3],
    pub direction: [f64; 3],
    /// Durable cross-session identity derived via UUIDv5 from entity
    /// PersistentIds. `null` for pre-PID solids (honest absence).
    pub pid: Option<String>,
    /// Reference datum for `"position"` kind records. `null` for all
    /// other kinds (additive — existing consumers see no change).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub datum: Option<DatumDescriptor>,
}

/// Wire shape for the single-call dimensioning answer: the callout-annotated
/// multi-view image AND the complete structured dimension table + cameras.
#[derive(Debug, Clone, Serialize)]
pub struct PartDimensionsResponse {
    pub png_base64: String,
    pub width: usize,
    pub height: usize,
    pub units: String,
    pub dims: DimsWire,
    pub dimensions: Vec<DimensionWire>,
    pub views: Vec<ViewProjectionWire>,
}

/// `GET /api/agent/parts/{id}/dimensions` — the dimensioning eye in one call.
///
/// Returns the EYE-1 multi-view with every analytic dimension drawn as a
/// leader+label callout, AND the complete structured table: each dimension's
/// id (the handle a future mould edits), kind, value, the face entities it
/// spans, and a 3D anchor. The image is the placed table; the JSON is
/// authoritative. Values are read off analytic surfaces / exact curves, never
/// the tessellation. Read lock only. `404` unknown id / empty tessellation.
pub async fn part_dimensions(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<PartDimensionsResponse>, StatusCode> {
    use base64::Engine as _;
    use geometry_engine::readable::extract_dimensions;
    use geometry_engine::render::dimensioned::{
        draw_dimension_callouts, render_dimensioned_multiview, Callout,
    };
    use geometry_engine::render::CanonicalView;
    use geometry_engine::tessellation::TessellationParams;

    let model = model_handle.read().await;
    let mut frame =
        render_dimensioned_multiview(&model, id as SolidId, &TessellationParams::default())
            .ok_or(StatusCode::NOT_FOUND)?;

    let dims = extract_dimensions(&model, id as SolidId);
    // The 5×7 overlay font has no Ø/∠/° glyphs — render ASCII, keep the pretty
    // label in the structured table.
    let callouts: Vec<Callout> = dims
        .iter()
        .map(|d| {
            let ascii: String = d
                .label
                .chars()
                .map(|c| match c {
                    'Ø' => 'D',
                    '∠' => 'A',
                    '°' => ' ',
                    o => o,
                })
                .collect();
            (d.anchor, ascii)
        })
        .collect();
    draw_dimension_callouts(&mut frame, &callouts);

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

    Ok(Json(PartDimensionsResponse {
        png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        width: frame.width,
        height: frame.height,
        units: frame.units.to_string(),
        dims: DimsWire {
            l: frame.dims.0,
            w: frame.dims.1,
            h: frame.dims.2,
        },
        dimensions: dims
            .into_iter()
            .map(|d| DimensionWire {
                id: d.id,
                kind: d.kind,
                value: d.value,
                unit: d.unit,
                label: d.label,
                entities: d.entities,
                anchor: d.anchor,
                direction: d.direction,
                pid: d.pid,
                datum: d.datum,
            })
            .collect(),
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

// ───────────────────── GD&T (kernel-verified conformance) ───────────

/// Body for attaching a GD&T tolerance to a feature: the kernel's own
/// [`geometry_engine::gdt::model::Annotation`] (a dimensional tolerance or a
/// feature control frame). Wire shape IS the kernel type — no DTO drift, exactly
/// like the rest of this handler module.
#[derive(Debug, Clone, Deserialize)]
pub struct AttachToleranceBody {
    pub annotation: geometry_engine::gdt::model::Annotation,
}

/// Response from attaching a tolerance: the durable persistent-id key the
/// annotation was filed under (hex), so the caller can re-reference the feature.
#[derive(Debug, Clone, Serialize)]
pub struct AttachToleranceResponse {
    pub success: bool,
    pub part_id: u32,
    pub feature_id: u32,
    /// The persistent-id key (as a 128-bit hex string) the annotation is bound to.
    pub annotation_key: String,
    pub kind: String,
}

/// `POST /api/agent/parts/{id}/faces/{face_id}/tolerance` — attach a dimensional
/// tolerance or feature control frame to a FACE. The annotation is bound to the
/// face's persistent id (minted if absent) so it survives regeneration. Write
/// lock (sidecar mutation). `404` on unknown face.
pub async fn attach_face_tolerance(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((id, face_id)): Path<(u32, u32)>,
    Json(body): Json<AttachToleranceBody>,
) -> Result<Json<AttachToleranceResponse>, StatusCode> {
    let mut model = model_handle.write().await;
    if model.faces.get(face_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let kind = body.annotation.kind_label().to_string();
    let key = model.attach_face_annotation(face_id, body.annotation);
    Ok(Json(AttachToleranceResponse {
        success: true,
        part_id: id,
        feature_id: face_id,
        annotation_key: format!("{:032x}", key.as_u128()),
        kind,
    }))
}

/// `POST /api/agent/parts/{id}/edges/{edge_id}/tolerance` — attach a tolerance to
/// an EDGE (for circularity / straightness form callouts). Write lock.
pub async fn attach_edge_tolerance(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((id, edge_id)): Path<(u32, u32)>,
    Json(body): Json<AttachToleranceBody>,
) -> Result<Json<AttachToleranceResponse>, StatusCode> {
    let mut model = model_handle.write().await;
    if model.edges.get(edge_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let kind = body.annotation.kind_label().to_string();
    let key = model.attach_edge_annotation(edge_id, body.annotation);
    Ok(Json(AttachToleranceResponse {
        success: true,
        part_id: id,
        feature_id: edge_id,
        annotation_key: format!("{:032x}", key.as_u128()),
        kind,
    }))
}

/// `GET /api/agent/parts/{id}/faces/{face_id}/verify` — the differentiator:
/// the KERNEL measures the actual geometry against every tolerance attached to
/// the face and returns one conformance result each. The verdict is computed
/// from the B-Rep, never asserted; an unimplemented characteristic reports
/// `not_yet_verified`, never a false pass. Read lock. Empty list when the face
/// carries no annotations.
pub async fn verify_face_tolerances(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((_id, face_id)): Path<(u32, u32)>,
) -> Result<Json<Vec<geometry_engine::gdt::verify::ConformanceResult>>, StatusCode> {
    let model = model_handle.read().await;
    if model.faces.get(face_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(model.verify_face_conformance(face_id)))
}

/// Query for edge verification: `?samples=N` controls the edge sampling density
/// (default 64, the resolution at which form deviation is measured along the
/// curve).
#[derive(Debug, Clone, Deserialize)]
pub struct VerifyEdgeQuery {
    pub samples: Option<usize>,
}

/// `GET /api/agent/parts/{id}/edges/{edge_id}/verify` — measure every edge form
/// tolerance (circularity / straightness) against the actual curve. Read lock.
pub async fn verify_edge_tolerances(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path((_id, edge_id)): Path<(u32, u32)>,
    Query(q): Query<VerifyEdgeQuery>,
) -> Result<Json<Vec<geometry_engine::gdt::verify::ConformanceResult>>, StatusCode> {
    let model = model_handle.read().await;
    if model.edges.get(edge_id).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let samples = q.samples.unwrap_or(64).clamp(2, 4096);
    Ok(Json(model.verify_edge_conformance(edge_id, samples)))
}

// ───────────────────── spatial primitives (point / ray / region) ────
//
// Thin Axum wrappers over the kernel's SDF-verified spatial-query core
// (`geometry-engine::queries::{field, raycast, region}`). The kernel is the
// single source of truth — these handlers add only HTTP framing, UUID/id
// resolution, and read-lock discipline. They never recompute geometry.

/// Request body for `POST /api/agent/parts/{id}/point-query`.
#[derive(Debug, Clone, Deserialize)]
pub struct PointQueryRequest {
    /// World-space query point `[x, y, z]`.
    pub point: [f64; 3],
}

/// Response for `point_query`: signed distance to the solid (negative inside,
/// positive outside, ~0 on the boundary), the inside/outside classification,
/// and the nearest boundary face + the exact point on it.
#[derive(Debug, Clone, Serialize)]
pub struct PointQueryResponse {
    pub solid_id: u32,
    pub point: Vec3Wire,
    /// Signed distance: negative inside the material, positive outside.
    pub signed_distance: f64,
    /// `true` when the query point lies inside the material.
    pub inside: bool,
    /// `inside` / `outside` / `on` — exact ray-parity classification.
    pub classification: String,
    /// Nearest boundary face id (recoverable to the B-Rep).
    pub nearest_face_id: u32,
    /// Exact closest point on that face.
    pub nearest_point: Vec3Wire,
    /// Distance to the nearest boundary point (unsigned).
    pub distance: f64,
}

/// `POST /api/agent/parts/{id}/point-query` — exact SDF probe of a single point.
///
/// Returns signed distance (negative inside / positive outside), the
/// inside/outside/on classification, and the nearest boundary face + point. All
/// values come from the kernel's analytic `signed_distance` / `nearest_on_solid`
/// (ray-parity sign, analytic nearest magnitude) — never a tessellation lookup.
/// Read lock only; `404` on unknown id or a degenerate solid with no reachable
/// boundary face.
pub async fn point_query(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(req): Json<PointQueryRequest>,
) -> Result<Json<PointQueryResponse>, StatusCode> {
    use geometry_engine::math::{Point3, Tolerance};
    use geometry_engine::queries::{classify_point, nearest_on_solid, signed_distance, PointClass};

    let sid = id as SolidId;
    let p = Point3::new(req.point[0], req.point[1], req.point[2]);

    let model = model_handle.read().await;
    if model.solids.get(sid).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    // Nearest boundary point + face (the recoverable handle) and the signed
    // distance share the same analytic nearest; compute both honestly.
    let (face_id, near_pt, dist) = nearest_on_solid(&model, sid, p).ok_or(StatusCode::NOT_FOUND)?;
    let (sd, _) = signed_distance(&model, sid, p).ok_or(StatusCode::NOT_FOUND)?;
    let class = classify_point(&model, sid, p, Tolerance::default().distance());
    let classification = match class {
        PointClass::Inside => "inside",
        PointClass::Outside => "outside",
        PointClass::On => "on",
    };

    Ok(Json(PointQueryResponse {
        solid_id: id,
        point: Vec3Wire {
            x: p.x,
            y: p.y,
            z: p.z,
        },
        signed_distance: sd,
        inside: sd < 0.0,
        classification: classification.to_string(),
        nearest_face_id: face_id,
        nearest_point: Vec3Wire {
            x: near_pt.x,
            y: near_pt.y,
            z: near_pt.z,
        },
        distance: dist,
    }))
}

/// Request body for `POST /api/agent/parts/{id}/ray-query`.
#[derive(Debug, Clone, Deserialize)]
pub struct RayQueryRequest {
    /// Ray origin `[x, y, z]`.
    pub origin: [f64; 3],
    /// Ray direction `[x, y, z]` (need not be unit length).
    pub direction: [f64; 3],
}

/// One ordered hit along the ray.
#[derive(Debug, Clone, Serialize)]
pub struct RayHitWire {
    pub face_id: u32,
    pub point: Vec3Wire,
    /// Outward-oriented surface normal at the hit.
    pub normal: Vec3Wire,
    /// Distance from the origin (ray parameter along the unit direction).
    pub distance: f64,
}

/// Response for `ray_query`: every face crossing along the ray, sorted near→far.
#[derive(Debug, Clone, Serialize)]
pub struct RayQueryResponse {
    pub solid_id: u32,
    pub origin: Vec3Wire,
    pub direction: Vec3Wire,
    pub hit_count: usize,
    pub hits: Vec<RayHitWire>,
}

/// `POST /api/agent/parts/{id}/ray-query` — analytic ray-cast against the solid.
///
/// Returns the ordered list of face crossings (face id, exact world point,
/// oriented normal, distance) from the kernel's `raycast_all` — exact analytic
/// surface intersections clipped to each face's real trim loops, never a mesh
/// approximation. A missing face renders as see-through (no phantom hit). Read
/// lock only; `404` on unknown id. An empty `hits` list means the ray missed.
pub async fn ray_query(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(req): Json<RayQueryRequest>,
) -> Result<Json<RayQueryResponse>, StatusCode> {
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::queries::raycast_all;

    let sid = id as SolidId;
    let origin = Point3::new(req.origin[0], req.origin[1], req.origin[2]);
    let direction = Vector3::new(req.direction[0], req.direction[1], req.direction[2]);

    let model = model_handle.read().await;
    if model.solids.get(sid).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }
    let hits: Vec<RayHitWire> = raycast_all(&model, sid, origin, direction)
        .into_iter()
        .map(|h| RayHitWire {
            face_id: h.face_id,
            point: Vec3Wire {
                x: h.point.x,
                y: h.point.y,
                z: h.point.z,
            },
            normal: Vec3Wire {
                x: h.normal.x,
                y: h.normal.y,
                z: h.normal.z,
            },
            distance: h.distance,
        })
        .collect();

    Ok(Json(RayQueryResponse {
        solid_id: id,
        origin: Vec3Wire {
            x: origin.x,
            y: origin.y,
            z: origin.z,
        },
        direction: Vec3Wire {
            x: direction.x,
            y: direction.y,
            z: direction.z,
        },
        hit_count: hits.len(),
        hits,
    }))
}

/// Region shape for `POST /api/agent/region-query`. Exactly one of `box`/
/// `sphere` describes the query volume.
#[derive(Debug, Clone, Deserialize)]
pub struct RegionQueryRequest {
    /// Box region: centre `[x,y,z]` + half-extents `[hx,hy,hz]`.
    #[serde(default)]
    pub center: Option<[f64; 3]>,
    #[serde(default)]
    pub half_extents: Option<[f64; 3]>,
    /// Sphere region: centre `[x,y,z]` (reuses `center`) + `radius`.
    #[serde(default)]
    pub radius: Option<f64>,
    /// Restrict to one solid; omit to scan every part in the scene.
    #[serde(default)]
    pub part_id: Option<u32>,
}

/// Per-part intersection result.
#[derive(Debug, Clone, Serialize)]
pub struct RegionPartHit {
    pub solid_id: u32,
    /// Face ids of this part whose world extent meets the query volume.
    pub face_ids: Vec<u32>,
}

/// Response for `region_query`: which parts/faces intersect the region and
/// whether the region is empty.
#[derive(Debug, Clone, Serialize)]
pub struct RegionQueryResponse {
    /// `box` or `sphere`.
    pub region: String,
    /// `true` when no part/face meets the query volume.
    pub empty: bool,
    pub parts: Vec<RegionPartHit>,
}

/// `POST /api/agent/region-query` — "what is in here?". Given an axis-aligned
/// box (`center` + `half_extents`) OR a sphere (`center` + `radius`), returns
/// the part/face ids whose sound world extent meets the volume, and whether the
/// region is empty. Operates over one part (`part_id`) or the whole scene.
///
/// Face extents come from the kernel's `faces_in_box` / `faces_in_sphere`
/// (exact trim-curve envelopes, never the mesh). Read lock only; `400` when the
/// region is under-specified (neither a complete box nor a sphere).
pub async fn region_query(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(req): Json<RegionQueryRequest>,
) -> Result<Json<RegionQueryResponse>, StatusCode> {
    use geometry_engine::math::Point3;
    use geometry_engine::queries::{faces_in_box, faces_in_sphere, WorldBox};

    let model = model_handle.read().await;

    // Which solids to scan: one part, or every part in the scene.
    let ids: Vec<SolidId> = match req.part_id {
        Some(pid) => {
            let sid = pid as SolidId;
            if model.solids.get(sid).is_none() {
                return Err(StatusCode::NOT_FOUND);
            }
            vec![sid]
        }
        None => model.solids.iter().map(|(id, _)| id).collect(),
    };

    // Resolve the query volume. A box needs both centre + half-extents; a sphere
    // needs centre + radius. Box wins when both are fully specified.
    let center = req.center;
    let (region_kind, eval): (&str, Box<dyn Fn(SolidId) -> Vec<u32>>) =
        match (center, req.half_extents, req.radius) {
            (Some(c), Some(h), _) => {
                let bx = WorldBox::from_center_half(
                    Point3::new(c[0], c[1], c[2]),
                    Point3::new(h[0].abs(), h[1].abs(), h[2].abs()),
                );
                let model_ref = &model;
                ("box", Box::new(move |sid| faces_in_box(model_ref, sid, bx)))
            }
            (Some(c), None, Some(r)) => {
                let centre = Point3::new(c[0], c[1], c[2]);
                let radius = r.abs();
                let model_ref = &model;
                (
                    "sphere",
                    Box::new(move |sid| faces_in_sphere(model_ref, sid, centre, radius)),
                )
            }
            _ => return Err(StatusCode::BAD_REQUEST),
        };

    let mut parts = Vec::new();
    for sid in ids {
        let face_ids = eval(sid);
        if !face_ids.is_empty() {
            parts.push(RegionPartHit {
                solid_id: sid,
                face_ids,
            });
        }
    }

    Ok(Json(RegionQueryResponse {
        region: region_kind.to_string(),
        empty: parts.is_empty(),
        parts,
    }))
}

// ─── Interactive measurement ──────────────────────────────────────────────────

/// One face reference in the `POST /api/agent/measure` body.
#[derive(Debug, Clone, Deserialize)]
pub struct MeasureSubjectWire {
    /// Kernel solid id (the `u32` from `list_parts` / `render_part`).
    pub part_id: u32,
    /// Always `"face"` for now; extensible to `"edge"` later.
    pub kind: String,
    /// Kernel face id.
    pub id: u32,
}

/// Request body for `POST /api/agent/measure`.
#[derive(Debug, Clone, Deserialize)]
pub struct MeasureRequest {
    pub a: MeasureSubjectWire,
    pub b: Option<MeasureSubjectWire>,
}

/// DimensionRecord-shaped response from `POST /api/agent/measure`.
///
/// Fields mirror [`DimensionWire`] so the frontend renders interactive
/// measurements with the same component as ambient dimensions.
/// `pid` is always `null` — interactive measurements are session-local
/// and have no durable identity to derive.
#[derive(Debug, Clone, Serialize)]
pub struct MeasureResponse {
    /// "distance" | "angle" | "diameter" | "face_info"
    pub kind: String,
    /// "plane_plane" | "axis_axis" | "axis_plane" | "nearest" | null
    pub relation: Option<String>,
    pub value: f64,
    /// "mm" | "deg"
    pub unit: String,
    /// Human label formatted like `readable/dimensions.rs`:
    /// `"Ø8.00"`, `"∠ 120.0°"`, `"10.00"`, `"A 123.4mm²"`.
    pub label: String,
    /// World-space anchor for the callout leader.
    pub anchor: [f64; 3],
    /// Unit direction the measurement is along (`null` for angles and
    /// face-info).
    pub direction: Option<[f64; 3]>,
    /// Face ids that participated in the measurement.
    pub entities: Vec<u32>,
    /// Always `null` — interactive measurements have no durable pid.
    pub pid: Option<String>,
}

/// Map a kernel [`MeasureResult`] to the wire shape, given the input face ids
/// and the document display unit.
///
/// Pure function — factored out so it can be unit-tested independently of the
/// async handler and the live router. `unit` must be the model's
/// `document_unit()` at call time; it drives label and `unit` field formatting.
/// Angles are always "deg" (no unit conversion).
pub fn map_measure_result(
    result: geometry_engine::queries::measure::MeasureResult,
    fa: u32,
    fb: Option<u32>,
    unit: geometry_engine::units::LengthUnit,
) -> MeasureResponse {
    use geometry_engine::queries::measure::MeasureResult;

    let mut entities: Vec<u32> = vec![fa];
    if let Some(b) = fb {
        entities.push(b);
    }

    match result {
        MeasureResult::Distance {
            value,
            anchor,
            direction,
            kind,
        } => {
            let label = unit.format_len(value);
            let unit_str = unit.suffix().to_string();
            MeasureResponse {
                kind: "distance".to_string(),
                relation: Some(kind.to_string()),
                value,
                unit: unit_str,
                label,
                anchor,
                direction: Some(direction),
                entities,
                pid: None,
            }
        }
        MeasureResult::Angle { degrees, anchor } => {
            let label = format!("\u{2220} {:.1}\u{00b0}", degrees);
            MeasureResponse {
                kind: "angle".to_string(),
                relation: None,
                value: degrees,
                unit: "deg".to_string(),
                label,
                anchor,
                direction: None,
                entities,
                pid: None,
            }
        }
        MeasureResult::Diameter {
            value,
            anchor,
            axis,
        } => {
            // Ø prefix (U+00D8) then the formatted length.
            let label = format!("\u{00d8}{}", unit.format_len(value));
            let unit_str = unit.suffix().to_string();
            MeasureResponse {
                kind: "diameter".to_string(),
                relation: None,
                value,
                unit: unit_str,
                label,
                anchor,
                direction: Some(axis),
                entities,
                pid: None,
            }
        }
        MeasureResult::FaceInfo {
            area,
            normal,
            anchor,
        } => {
            // "A " prefix then formatted area (e.g. "A 2.48in²").
            let formatted_area = unit.format_area(area);
            let label = format!("A {}", formatted_area);
            // Unit for area = suffix + "²".
            let unit_str = format!("{}²", unit.suffix());
            MeasureResponse {
                kind: "face_info".to_string(),
                relation: None,
                value: area,
                unit: unit_str,
                label,
                anchor,
                direction: normal,
                entities,
                pid: None,
            }
        }
    }
}

/// `POST /api/agent/measure` — kernel-exact face-pair (or single-face)
/// measurement.
///
/// Body: `{ "a": {"part_id": u32, "kind": "face", "id": u32},
///          "b": {"part_id": u32, "kind": "face", "id": u32} | null }`.
///
/// A single cylindrical face returns `Diameter`; a single planar face
/// returns `FaceInfo` with area + normal. A face pair resolves to the
/// best analytic relation (plane‖plane → distance, plane∠plane → dihedral
/// angle, cyl↔cyl → axis-axis distance, cyl↔plane → axis-plane distance,
/// fallback → nearest-point).
///
/// The response is DimensionRecord-shaped so the frontend renders
/// interactive measurements with the same component as ambient dimensions.
///
/// 404 — solid or face not found in the active model.
/// 422 — face pair does not admit the requested measurement; the kernel's
///        actionable reason is surfaced verbatim (never a guessed number).
pub async fn measure(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Json(req): Json<MeasureRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    use geometry_engine::primitives::face::FaceId;
    use geometry_engine::primitives::solid::SolidId;
    use geometry_engine::queries::measure::measure as kernel_measure;
    use geometry_engine::queries::{MeasureError, MeasureSubject};

    if req.a.kind != "face" {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "unsupported_measure",
                "reason": format!(
                    "Subject kind {:?} is not supported; only \"face\" is accepted.",
                    req.a.kind
                ),
            })),
        );
    }
    if let Some(ref b) = req.b {
        if b.kind != "face" {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "unsupported_measure",
                    "reason": format!(
                        "Subject kind {:?} is not supported; only \"face\" is accepted.",
                        b.kind
                    ),
                })),
            );
        }
    }

    let subj_a = MeasureSubject::Face {
        solid: req.a.part_id as SolidId,
        face: req.a.id as FaceId,
    };
    let subj_b = req.b.as_ref().map(|b| MeasureSubject::Face {
        solid: b.part_id as SolidId,
        face: b.id as FaceId,
    });

    let fa_id = req.a.id;
    let fb_id = req.b.as_ref().map(|b| b.id);

    // Write lock: `measure` takes `&mut BRepModel` to warm the area cache on
    // first call (matches the pattern established by `query_face`).
    let mut model = model_handle.write().await;
    let doc_unit = model.document_unit();
    match kernel_measure(&mut model, subj_a, subj_b) {
        Ok(result) => {
            let wire = map_measure_result(result, fa_id, fb_id, doc_unit);
            // `MeasureResponse` contains only f64/String/Option — serialization
            // to `serde_json::Value` cannot fail for these primitive types.
            // `unwrap_or_else` (not `unwrap`) is the safe fallback pattern.
            let body = serde_json::to_value(&wire).unwrap_or_else(
                |e| serde_json::json!({ "error": "serialization_failed", "reason": e.to_string() }),
            );
            (StatusCode::OK, Json(body))
        }
        Err(MeasureError::NotFound { solid, face }) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "not_found",
                "reason": format!(
                    "Face {face} is not part of solid {solid} — \
                     it may have been consumed by a later operation, \
                     or the solid/face id is unknown.",
                ),
            })),
        ),
        Err(MeasureError::Unsupported { reason }) => (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "unsupported_measure",
                "reason": reason,
            })),
        ),
    }
}
