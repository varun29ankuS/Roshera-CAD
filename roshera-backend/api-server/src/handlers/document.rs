//! Document-level settings — unit system.
//!
//! `GET /api/document/units` — read the current display unit.
//! `PATCH /api/document/units` — set the display unit.
//!
//! The unit governs how every label/formatted value is expressed; the
//! kernel geometry is always mm-native. Setting the unit is instant and
//! lossless. It is NOT a timeline event (display metadata only).

use crate::part_mgr::ActiveModel;
use axum::{http::StatusCode, Json};
use geometry_engine::units::LengthUnit;
use serde::{Deserialize, Serialize};

/// Response shape for `GET /api/document/units` and the success response of
/// `PATCH /api/document/units`.
#[derive(Debug, Clone, Serialize)]
pub struct DocumentUnitsResponse {
    /// The current display unit token: `"mm"`, `"cm"`, `"m"`, `"in"`, or `"ft"`.
    pub unit: &'static str,
}

impl DocumentUnitsResponse {
    fn from_unit(u: LengthUnit) -> Self {
        Self { unit: u.suffix() }
    }
}

/// `GET /api/document/units` — return the model's current display unit.
///
/// The response is `{"unit": "mm"|"cm"|"m"|"in"|"ft"}`. Read lock only;
/// never a timeline event.
pub async fn get_document_units(
    ActiveModel(model_handle): ActiveModel,
) -> Json<DocumentUnitsResponse> {
    let model = model_handle.read().await;
    Json(DocumentUnitsResponse::from_unit(model.document_unit()))
}

/// Request body for `PATCH /api/document/units`.
#[derive(Debug, Clone, Deserialize)]
pub struct PatchDocumentUnitsRequest {
    pub unit: String,
}

/// Error body for a 400 on an unrecognised unit token.
#[derive(Debug, Clone, Serialize)]
pub struct InvalidUnitError {
    pub error: &'static str,
    pub reason: String,
}

/// `PATCH /api/document/units` — set the model's display unit.
///
/// Body: `{"unit": "<token>"}` where `<token>` is one of `mm`, `cm`, `m`,
/// `in`, `ft` (case-insensitive; long forms like `"inch"` / `"feet"` are also
/// accepted via [`LengthUnit::parse`]). On success returns the new state
/// `{"unit":"<suffix>"}`. On an unrecognised token returns `400` with
/// `{"error":"invalid_unit","reason":"..."}` listing the valid tokens.
///
/// NOT a timeline event — display metadata only; switching units is instant
/// and lossless.
pub async fn patch_document_units(
    ActiveModel(model_handle): ActiveModel,
    Json(req): Json<PatchDocumentUnitsRequest>,
) -> Result<Json<DocumentUnitsResponse>, (StatusCode, Json<InvalidUnitError>)> {
    let parsed = LengthUnit::parse(&req.unit).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(InvalidUnitError {
                error: "invalid_unit",
                reason: format!(
                    "{:?} is not a recognised unit token. \
                     Valid tokens: mm, cm, m, in, ft.",
                    req.unit
                ),
            }),
        )
    })?;
    let mut model = model_handle.write().await;
    model.set_document_unit(parsed);
    Ok(Json(DocumentUnitsResponse::from_unit(parsed)))
}
