//! Datum entity endpoints — Origin, reference planes, reference axes.
//!
//! Slice 1 surfaces the kernel's `DatumStore` to the frontend so the
//! model tree can render the canonical seven defaults (Origin + three
//! planes + three axes) and the viewport can use them as snap targets.
//!
//! Read-only for now: this slice does not allow create / rename / delete
//! of datums. User-authored datums arrive in Slice 3.

use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use geometry_engine::primitives::datum::{AxisDirection, Datum, DatumKind};
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
    let anchor = solid.anchor.as_ref().ok_or(StatusCode::NOT_FOUND)?;
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
    match model.datums.set_visible(id, req.visible) {
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
