//! Datum entity endpoints — Origin, reference planes, reference axes.
//!
//! Slice 1 surfaces the kernel's `DatumStore` to the frontend so the
//! model tree can render the canonical seven defaults (Origin + three
//! planes + three axes) and the viewport can use them as snap targets.
//!
//! Slice 4a adds user-authored datum CRUD: `POST /api/datums`,
//! `PATCH /api/datums/:id`, `DELETE /api/datums/:id`. Defaults remain
//! read-only — rename / set_transform / delete on a default returns
//! `409 Conflict`.

use crate::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use geometry_engine::math::{Matrix4, Point3};
use geometry_engine::primitives::datum::{
    AxisDirection, Datum, DatumError, DatumKind,
};
use geometry_engine::primitives::solid::SolidAnchor;
use geometry_engine::sketch2d::sketch_plane::PlaneOrientation;
use serde::{Deserialize, Serialize};

/// Wire-format kind tag. Mirrors `DatumKind` but flattened for JSON
/// consumption by the frontend without leaking enum-variant tags.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DatumKindWire {
    Origin,
    Plane,
    Axis,
}

/// Wire-format datum DTO sent to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatumDto {
    /// Stable kernel id.
    pub id: u32,
    /// User-visible name.
    pub name: String,
    /// What kind of reference this datum represents.
    pub kind: DatumKindWire,
    /// For `Plane`, one of "xy" / "xz" / "yz" / "custom".
    /// `None` for `Origin` / `Axis`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plane_orientation: Option<String>,
    /// For `Axis`, one of "x" / "y" / "z". `None` for `Origin` / `Plane`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub axis_direction: Option<String>,
    /// World-space anchor point as `[x, y, z]`.
    pub origin: [f64; 3],
    /// Whether the datum is rendered.
    pub visible: bool,
    /// Whether the datum was seeded automatically (not user-deletable).
    pub is_default: bool,
}

impl From<Datum> for DatumDto {
    fn from(datum: Datum) -> Self {
        let (kind, plane_orientation, axis_direction) = match datum.kind {
            DatumKind::Origin => (DatumKindWire::Origin, None, None),
            DatumKind::Plane(orient) => {
                let label = match orient {
                    PlaneOrientation::XY => "xy",
                    PlaneOrientation::XZ => "xz",
                    PlaneOrientation::YZ => "yz",
                    PlaneOrientation::Custom => "custom",
                };
                (DatumKindWire::Plane, Some(label.to_string()), None)
            }
            DatumKind::Axis(dir) => {
                let label = match dir {
                    AxisDirection::X => "x",
                    AxisDirection::Y => "y",
                    AxisDirection::Z => "z",
                };
                (DatumKindWire::Axis, None, Some(label.to_string()))
            }
        };

        Self {
            id: datum.id,
            name: datum.name,
            kind,
            plane_orientation,
            axis_direction,
            origin: [datum.origin.x, datum.origin.y, datum.origin.z],
            visible: datum.visible,
            is_default: datum.is_default,
        }
    }
}

/// Response payload for `GET /api/datums`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DatumListResponse {
    /// All datums, ordered by id.
    pub datums: Vec<DatumDto>,
}

/// `GET /api/datums` — list every datum in the active model.
pub async fn list_datums(State(state): State<AppState>) -> Json<DatumListResponse> {
    tracing::debug!("Listing datums");
    let model = state.model.read().await;
    let datums = model
        .datums
        .snapshot()
        .into_iter()
        .map(DatumDto::from)
        .collect();
    Json(DatumListResponse { datums })
}

/// Request payload for `PATCH /api/datums/:id/visibility`.
#[derive(Debug, Clone, Deserialize)]
pub struct SetDatumVisibilityRequest {
    pub visible: bool,
}

/// Response payload for visibility updates.
#[derive(Debug, Clone, Serialize)]
pub struct SetDatumVisibilityResponse {
    pub id: u32,
    pub visible: bool,
}

/// Wire-format anchor DTO returned by `GET /api/solids/:id/anchor`.
///
/// Carries enough context that a UI can render "this solid is placed
/// against `<datum_name>`" without re-querying the datum store. The
/// `local_transform` is a row-major flattened 4×4 (16 f64s) — same
/// convention `Matrix4::serialize` uses elsewhere in the API.
#[derive(Debug, Clone, Serialize)]
pub struct SolidAnchorDto {
    pub solid_id: u32,
    pub datum_id: u32,
    pub datum_name: String,
    pub local_transform: [[f64; 4]; 4],
}

/// `GET /api/solids/:id/anchor` — return anchor metadata for the solid,
/// or `404` when the solid is unknown / unanchored.
pub async fn get_solid_anchor(
    State(state): State<AppState>,
    Path(id): Path<u32>,
) -> Result<Json<SolidAnchorDto>, StatusCode> {
    let model = state.model.read().await;
    let solid = model.solids.get(id).ok_or(StatusCode::NOT_FOUND)?;
    let anchor = &solid.anchor;
    let datum = model
        .datums
        .get(anchor.datum_id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let m = &anchor.local_transform;
    let local_transform = [
        [m.get(0, 0), m.get(0, 1), m.get(0, 2), m.get(0, 3)],
        [m.get(1, 0), m.get(1, 1), m.get(1, 2), m.get(1, 3)],
        [m.get(2, 0), m.get(2, 1), m.get(2, 2), m.get(2, 3)],
        [m.get(3, 0), m.get(3, 1), m.get(3, 2), m.get(3, 3)],
    ];
    Ok(Json(SolidAnchorDto {
        solid_id: id,
        datum_id: anchor.datum_id,
        datum_name: datum.name,
        local_transform,
    }))
}

/// `PATCH /api/datums/:id/visibility` — toggle a datum's visibility.
///
/// Returns `404` when the datum id is unknown. The kernel is the source
/// of truth for visibility (the field lives on `Datum`, not in
/// frontend-only state) so refreshes preserve the toggle.
pub async fn set_datum_visibility(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(req): Json<SetDatumVisibilityRequest>,
) -> Result<Json<SetDatumVisibilityResponse>, StatusCode> {
    let model = state.model.read().await;
    match model.set_datum_visibility(id, req.visible) {
        Some(_prev) => Ok(Json(SetDatumVisibilityResponse {
            id,
            visible: req.visible,
        })),
        None => {
            tracing::warn!("set_datum_visibility: unknown datum id {}", id);
            Err(StatusCode::NOT_FOUND)
        }
    }
}

// ───────────────────── Slice 4a: user-authored CRUD ──────────────────────────

/// Request body for `POST /api/datums`. Tagged on the lowercase `kind`
/// discriminator. Each variant carries exactly the data required to
/// build the corresponding kernel call:
/// - `plane`: a 4×4 row-major `transform` whose local +Z is the plane normal.
/// - `axis`:  an `origin` point and a canonical `direction` (`x`/`y`/`z`).
/// - `point`: a world `position`.
///
/// Slice 4b will extend this with a `source` discriminator for derived
/// datums (`offset_plane`, `three_points`, …) — they share this
/// endpoint so the frontend has a single creation entry point.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum CreateDatumRequest {
    Plane {
        name: String,
        /// Row-major 4×4 transform.
        transform: [[f64; 4]; 4],
    },
    Axis {
        name: String,
        origin: [f64; 3],
        /// One of "x", "y", "z" (lowercase). Slice 4a restricts user
        /// axes to canonical directions; arbitrary directions live in
        /// slice 4b's derived datums.
        direction: String,
    },
    Point {
        name: String,
        position: [f64; 3],
    },
}

/// Response for create / update — the canonical `DatumDto` for the
/// affected datum.
#[derive(Debug, Clone, Serialize)]
pub struct DatumMutationResponse {
    pub datum: DatumDto,
}

/// Request body for `PATCH /api/datums/:id`. Both fields are optional;
/// omitted fields leave the corresponding kernel state unchanged. An
/// empty body is rejected with `400` — `PATCH` with nothing to do is a
/// client bug.
#[derive(Debug, Clone, Deserialize)]
pub struct UpdateDatumRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transform: Option<[[f64; 4]; 4]>,
}

/// Query parameters for `DELETE /api/datums/:id`.
///
/// `cascade=detach` re-anchors every dependent solid to the world
/// `Origin` datum (id 0) before removing the target datum. Without
/// `cascade`, the kernel returns `409 Conflict` listing the dependents.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DeleteDatumQuery {
    #[serde(default)]
    pub cascade: Option<String>,
}

/// Response body for `DELETE /api/datums/:id`. Reports which solids (if
/// any) were detached as part of the cascade so the frontend can
/// refresh their anchors without an extra round-trip.
#[derive(Debug, Clone, Serialize)]
pub struct DeleteDatumResponse {
    pub datum_id: u32,
    pub detached_solids: Vec<u32>,
}

/// Map a kernel `DatumError` to an HTTP status. `EmptyName` and
/// invalid-input errors that originate from the kernel layer are 400;
/// `UnknownId` is 404; `DefaultDatumNotMutable` is 409.
fn map_datum_error(err: DatumError) -> StatusCode {
    match err {
        DatumError::EmptyName => StatusCode::BAD_REQUEST,
        DatumError::UnknownId(_) => StatusCode::NOT_FOUND,
        DatumError::DefaultDatumNotMutable(_) => StatusCode::CONFLICT,
    }
}

/// `POST /api/datums` — create a user-authored datum.
///
/// Returns `201 Created` with the created `DatumDto` on success.
/// `400` for empty name or unrecognized axis direction. The created
/// datum has `is_default = false`.
pub async fn create_datum(
    State(state): State<AppState>,
    Json(req): Json<CreateDatumRequest>,
) -> Result<(StatusCode, Json<DatumMutationResponse>), StatusCode> {
    let model = state.model.read().await;
    let id = match req {
        CreateDatumRequest::Plane { name, transform } => {
            let m = Matrix4::from_rows_array(transform);
            model
                .create_datum_plane(name, m)
                .map_err(map_datum_error)?
        }
        CreateDatumRequest::Axis {
            name,
            origin,
            direction,
        } => {
            let dir = match direction.to_ascii_lowercase().as_str() {
                "x" => AxisDirection::X,
                "y" => AxisDirection::Y,
                "z" => AxisDirection::Z,
                other => {
                    tracing::warn!(
                        "create_datum: unrecognized axis direction {:?}",
                        other
                    );
                    return Err(StatusCode::BAD_REQUEST);
                }
            };
            let origin_pt = Point3::new(origin[0], origin[1], origin[2]);
            model
                .create_datum_axis(name, origin_pt, dir)
                .map_err(map_datum_error)?
        }
        CreateDatumRequest::Point { name, position } => {
            let pos = Point3::new(position[0], position[1], position[2]);
            model
                .create_datum_point(name, pos)
                .map_err(map_datum_error)?
        }
    };

    let datum = model
        .datums
        .get(id)
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok((
        StatusCode::CREATED,
        Json(DatumMutationResponse {
            datum: DatumDto::from(datum),
        }),
    ))
}

/// `PATCH /api/datums/:id` — rename and/or replace transform of a
/// user-authored datum.
///
/// `404` when the id is unknown, `409` when the target is a seeded
/// default, `400` for empty body or empty name.
pub async fn update_datum(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Json(req): Json<UpdateDatumRequest>,
) -> Result<Json<DatumMutationResponse>, StatusCode> {
    if req.name.is_none() && req.transform.is_none() {
        tracing::warn!("update_datum: empty patch body for id {}", id);
        return Err(StatusCode::BAD_REQUEST);
    }

    let model = state.model.read().await;
    if let Some(name) = req.name {
        model.rename_datum(id, name).map_err(map_datum_error)?;
    }
    if let Some(transform) = req.transform {
        let m = Matrix4::from_rows_array(transform);
        model
            .set_datum_transform(id, m)
            .map_err(map_datum_error)?;
    }

    let datum = model.datums.get(id).ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(DatumMutationResponse {
        datum: DatumDto::from(datum),
    }))
}

/// `DELETE /api/datums/:id` — remove a user-authored datum.
///
/// `404` when unknown, `409` when the target is a seeded default or
/// when there are dependent solids and `?cascade=detach` was not
/// supplied. With `cascade=detach`, each dependent solid is
/// re-anchored to `Origin` before the datum is removed.
pub async fn delete_datum(
    State(state): State<AppState>,
    Path(id): Path<u32>,
    Query(q): Query<DeleteDatumQuery>,
) -> Result<Json<DeleteDatumResponse>, StatusCode> {
    let cascade = matches!(
        q.cascade.as_deref().map(str::to_ascii_lowercase).as_deref(),
        Some("detach")
    );

    let mut model = state.model.write().await;

    // Default + existence checks happen inside `delete_datum`, but we
    // need to enumerate dependents first so the 409 path returns
    // useful detail.
    let datum = model
        .datums
        .get(id)
        .ok_or(StatusCode::NOT_FOUND)?;
    if datum.is_default {
        return Err(StatusCode::CONFLICT);
    }
    drop(datum);

    let dependents: Vec<u32> = model
        .solids
        .iter()
        .filter_map(|(sid, s)| (s.anchor.datum_id == id).then_some(sid))
        .collect();

    if !dependents.is_empty() && !cascade {
        tracing::warn!(
            "delete_datum: refusing to delete id {} with {} dependents (no cascade)",
            id,
            dependents.len()
        );
        return Err(StatusCode::CONFLICT);
    }

    // Re-anchor each dependent to Origin before removing the datum.
    for sid in &dependents {
        if let Some(solid) = model.solids.get_mut(*sid) {
            solid.anchor = SolidAnchor::world_origin();
        }
    }

    model.delete_datum(id).map_err(map_datum_error)?;

    Ok(Json(DeleteDatumResponse {
        datum_id: id,
        detached_solids: dependents,
    }))
}
