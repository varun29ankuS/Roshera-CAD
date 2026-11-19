//! Annotating a sheet's dimensions: tolerance, text, move, suppress.
//!
//! The value of a dimension is measured; `POST .../dimensions/{dim_id}`
//! (`drawing_edit.rs`) changes the PART. These routes change what stands
//! beside the value on the SHEET, for a person in the drawing tab and for an
//! agent alike:
//!
//! * `GET   /api/drawings/{id}/dimensions` -- every dimension as inked, what
//!   is authored on it, and the dimensions the sheet leaves out;
//! * `PATCH /api/drawings/{id}/dimensions/{dim}` -- set or clear the
//!   tolerance, the text before / after the value, the move, the
//!   suppression. `{dim}` is a dimension id (`data-dim-id` in the SVG) or a
//!   dimension key;
//! * `GET   /api/iso286?nominal=&class=` -- the limit deviations of an ISO
//!   286 class (or a hole/shaft pair) at a size, from the kernel's tables.
//!
//! Every change is one recorded `drawing.dimension.update` event carrying the
//! dimension's durable key and the whole edit, so it is undone by the
//! timeline and replayed onto the rebuilt sheet
//! (`timeline_engine::drawing_history`). Refused, never approximated: a
//! dimension without a persistent identity, a tolerance that is not one, an
//! ISO 286 class outside the tables, an unsigned suppression.

use axum::{
    extract::{Path, Query, State},
    response::Json,
};
use serde::{Deserialize, Deserializer};
use uuid::Uuid;

use geometry_engine::drawing::dimension_edit::{
    dimension_ink, DimTolerance, DimensionEdit, DimensionEditError, Waiver,
};
use geometry_engine::drawing::types::Drawing;
use geometry_engine::drawing::SheetReadbackCertificate;
use geometry_engine::gdt::iso286::{self, FitRefusal};
use geometry_engine::operations::recorder::RecordedOperation;

use crate::drawing_mgr::{certify_off_lock, resolve_owner_model, RecordedOperationDrawingExt};
use crate::error_catalog::{ApiError, ErrorCode};
use crate::AppState;

/// The history event one dimension annotation is recorded as.
pub const DIMENSION_UPDATE_EVENT: &str = "drawing.dimension.update";

fn drawing_not_found(id: Uuid) -> ApiError {
    ApiError::new(ErrorCode::SolidNotFound, format!("drawing {id} not found"))
}

/// A refused annotation on the wire: the kernel's reason, under one gate.
fn edit_refusal(drawing: Uuid, dimension: &str, e: DimensionEditError) -> ApiError {
    let (code, reason) = match &e {
        DimensionEditError::NoSuchDimension(_) => (ErrorCode::SolidNotFound, "no_such_dimension"),
        DimensionEditError::NotAddressable(_) => (ErrorCode::InvalidParameter, "not_addressable"),
        DimensionEditError::ToleranceNotSupported { .. } => {
            (ErrorCode::InvalidParameter, "tolerance_not_supported")
        }
        DimensionEditError::BadTolerance(_) => (ErrorCode::InvalidParameter, "bad_tolerance"),
        DimensionEditError::Fit(_) => (ErrorCode::InvalidParameter, "fit_out_of_scope"),
        DimensionEditError::BadText(_) => (ErrorCode::InvalidParameter, "bad_text"),
        DimensionEditError::BadShift => (ErrorCode::InvalidParameter, "bad_move"),
        DimensionEditError::UnsignedWaiver => (ErrorCode::InvalidParameter, "unsigned_suppression"),
    };
    ApiError::new(code, e.to_string()).with_details(serde_json::json!({
        "gate": "dimension_annotation",
        "reason": reason,
        "drawing_id": drawing,
        "dimension": dimension,
    }))
}

/// A field that can be absent (leave as it is), `null` (clear) or a value.
fn present<'de, D, T>(de: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(de).map(Some)
}

/// Body of `PATCH /api/drawings/{id}/dimensions/{dim}`. Every field is
/// optional: an absent field is left as it is, `null` clears it.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnnotateDimensionRequest {
    /// `{"type":"symmetric","pm":0.1}`, `{"type":"bilateral","upper":0.05,
    /// "lower":-0.1}`, `{"type":"limits","upper":25.15,"lower":24.85}`,
    /// `{"type":"fit","designation":"H7"}`, `{"type":"basic"}`,
    /// `{"type":"reference"}`; `null` removes the tolerance.
    #[serde(default, deserialize_with = "present")]
    pub tolerance: Option<Option<DimTolerance>>,
    /// Text before the value (`"4X"`).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Text after the value (`"TYP"`, `"M12x1.75-6H"`).
    #[serde(default)]
    pub suffix: Option<String>,
    /// Where the text / dimension line was dragged to, relative to where the
    /// layout puts it: sheet millimetres `[right, down]`. `null` puts it back.
    #[serde(default, deserialize_with = "present")]
    pub text_shift_mm: Option<Option<[f64; 2]>>,
    /// `{"by":"<name>","reason":"..."}` takes the dimension off the sheet
    /// (the certificate lists it as a waived omission naming `by`); `null`
    /// puts it back.
    #[serde(default, deserialize_with = "present")]
    pub suppressed: Option<Option<Waiver>>,
}

impl AnnotateDimensionRequest {
    /// `current` with this request's fields applied.
    fn applied_to(&self, current: Option<&DimensionEdit>) -> DimensionEdit {
        let mut edit = current.cloned().unwrap_or_default();
        if let Some(t) = &self.tolerance {
            edit.tolerance = t.clone();
        }
        if let Some(p) = &self.prefix {
            edit.prefix = p.trim().to_string();
        }
        if let Some(s) = &self.suffix {
            edit.suffix = s.trim().to_string();
        }
        if let Some(shift) = self.text_shift_mm {
            // A zero move is no move.
            edit.text_shift_mm = shift.filter(|s| s[0].abs() > 1e-9 || s[1].abs() > 1e-9);
        }
        if let Some(w) = &self.suppressed {
            edit.suppressed = w.clone();
        }
        edit
    }

    fn is_empty(&self) -> bool {
        self.tolerance.is_none()
            && self.prefix.is_none()
            && self.suffix.is_none()
            && self.text_shift_mm.is_none()
            && self.suppressed.is_none()
    }
}

/// One dimension as the property panel / an agent reads it.
fn dimension_json(drawing: &Drawing, view_name: &str, placed: bool, id: &str) -> serde_json::Value {
    let Some(dim) = drawing.find_dimension(id) else {
        return serde_json::Value::Null;
    };
    let edit = dim
        .key
        .as_ref()
        .and_then(|k| drawing.dimension_edits.get(k));
    let ink = dimension_ink(dim, edit, drawing.document_unit);
    serde_json::json!({
        "id": dim.id,
        "key": dim.key,
        "view": view_name,
        "kind": dim.kind,
        "value": dim.value,
        "unit": dim.unit,
        // The measured label, and the label as inked with what is authored.
        "label": dim.label,
        "ink": ink.label,
        "editable": dim.key.is_some(),
        "placed": placed,
        "suppressed": edit.is_some_and(|e| e.suppressed.is_some()),
        "edit": edit,
        "tolerance": ink.tolerance,
        "authored_text": ink.authored,
        "unresolved": ink.unresolved,
    })
}

/// The certificate fields a client shows after an edit: one chip.
fn certificate_summary(cert: &SheetReadbackCertificate) -> serde_json::Value {
    serde_json::json!({
        "sound": cert.sound,
        "complete": cert.complete,
        "counts": cert.counts,
        "omission_summary": cert.omission_summary,
        "omissions": cert.omissions,
    })
}

/// The sheet's certificate against its owner's live model, when the owner
/// resolves. `None` (never a guess) when it does not.
async fn certificate_of(
    state: &AppState,
    id: Uuid,
    drawing: Drawing,
) -> Result<Option<SheetReadbackCertificate>, ApiError> {
    let Some((owner, _)) = state.drawings.get_with_owner(&id) else {
        return Err(drawing_not_found(id));
    };
    match resolve_owner_model(state, &owner).await {
        Some(model) => Ok(Some(certify_off_lock(model, drawing).await?)),
        None => Ok(None),
    }
}

/// `GET /api/drawings/{id}/dimensions` -- every dimension the sheet carries
/// (as inked, with what is authored on it) and every dimension it leaves out.
pub async fn list_dimensions(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let handle = state
        .drawings
        .get(&id)
        .ok_or_else(|| drawing_not_found(id))?;
    let drawing = handle.read().await.clone();
    let layout = geometry_engine::drawing::layout::compute_layout(&drawing);
    let mut dimensions = Vec::new();
    for (vi, view) in drawing.views.iter().enumerate() {
        for dim in &view.dimensions {
            let placed = layout
                .dimensions
                .iter()
                .any(|p| p.owner_view == vi && p.dim_id == dim.id);
            dimensions.push(dimension_json(&drawing, &view.name, placed, &dim.id));
        }
    }
    let certificate = certificate_of(&state, id, drawing).await?;
    Ok(Json(serde_json::json!({
        "drawing_id": id,
        "dimensions": dimensions,
        "certificate": certificate.as_ref().map(certificate_summary),
    })))
}

/// `PATCH /api/drawings/{id}/dimensions/{dim}` -- annotate one dimension.
///
/// `{dim}` is a dimension id on the sheet, or a dimension key. A key that is
/// on no view is accepted only for a dimension the certificate reports as
/// omitted, and only to suppress it (sign for the gap) or lift that
/// suppression.
///
/// The response gives the dimension as now inked and the sheet's certificate
/// summary (`sound`, `complete`, the omissions), so a client shows the effect
/// of the edit without a second call.
pub async fn annotate_dimension(
    State(state): State<AppState>,
    Path((id, dim)): Path<(Uuid, String)>,
    Json(req): Json<AnnotateDimensionRequest>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if req.is_empty() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "nothing to change: give tolerance, prefix, suffix, text_shift_mm or suppressed",
        ));
    }
    let handle = state
        .drawings
        .get(&id)
        .ok_or_else(|| drawing_not_found(id))?;

    // A reference that is on no view may be the key of an omitted dimension:
    // ask the certificate, off the sheet lock.
    let snapshot = handle.read().await.clone();
    let on_sheet = snapshot.find_dimension(&dim).is_some();
    let known_omitted_key = if on_sheet {
        false
    } else {
        certificate_of(&state, id, snapshot)
            .await?
            .is_some_and(|c| {
                c.omissions
                    .iter()
                    .any(|o| o.key.as_deref() == Some(dim.as_str()))
            })
    };

    let (key, edit, shown, after) = {
        let mut guard = handle.write().await;
        let edit = req.applied_to(guard.dimension_edit(&dim));
        let key = guard
            .set_dimension_edit(&dim, edit.clone(), known_omitted_key)
            .map_err(|e| edit_refusal(id, &dim, e))?;
        let view_name = guard
            .views
            .iter()
            .find(|v| {
                v.dimensions
                    .iter()
                    .any(|d| d.key.as_deref() == Some(key.as_str()))
            })
            .map(|v| v.name.clone())
            .unwrap_or_default();
        let shown = if on_sheet {
            let layout = geometry_engine::drawing::layout::compute_layout(&guard);
            let placed = guard
                .find_dimension(&key)
                .is_some_and(|d| layout.dimensions.iter().any(|p| p.dim_id == d.id));
            dimension_json(&guard, &view_name, placed, &key)
        } else {
            serde_json::json!({ "key": key, "placed": false, "edit": edit })
        };
        (key, edit, shown, guard.clone())
    };

    state.drawings.record_event(
        RecordedOperation::new(DIMENSION_UPDATE_EVENT)
            .with_parameters(serde_json::json!({
                "key": key,
                "dimension_id": dim,
                "edit": edit,
            }))
            .with_input_drawing(id),
    );

    let certificate = certificate_of(&state, id, after).await?;
    Ok(Json(serde_json::json!({
        "success": true,
        "drawing_id": id,
        "dimension": shown,
        "certificate": certificate.as_ref().map(certificate_summary),
    })))
}

/// Query of `GET /api/iso286`.
#[derive(Debug, Deserialize)]
pub struct Iso286Query {
    /// Nominal size, millimetres.
    pub nominal: f64,
    /// One class (`H7`, `g6`) or a hole/shaft pair (`H7/g6`).
    pub class: String,
}

fn fit_refusal(e: FitRefusal) -> ApiError {
    let reason = match &e {
        FitRefusal::BadDesignation(_) => "bad_designation",
        FitRefusal::NominalOutOfRange { .. } => "nominal_out_of_range",
        FitRefusal::UnsupportedClass { .. } => "class_not_tabulated",
        FitRefusal::WrongArity(_) => "bad_designation",
    };
    ApiError::new(ErrorCode::InvalidParameter, e.to_string()).with_details(serde_json::json!({
        "gate": "iso286",
        "reason": reason,
    }))
}

fn class_json(nominal: f64, c: &iso286::ResolvedClass) -> serde_json::Value {
    let (lower, upper) = c.limits(nominal);
    serde_json::json!({
        "class": c.class,
        "kind": if c.hole { "hole" } else { "shaft" },
        "grade": c.grade,
        "upper_deviation_mm": c.upper_mm,
        "lower_deviation_mm": c.lower_mm,
        "upper_limit_mm": upper,
        "lower_limit_mm": lower,
    })
}

/// `GET /api/iso286?nominal=25&class=H7` -- the limit deviations and limits
/// of size of an ISO 286 class, or for a pair (`H7/g6`) both classes, the
/// extreme clearances and whether the fit is a clearance, transition or
/// interference fit. Outside the kernel's tables: a typed refusal, never an
/// interpolated number.
pub async fn iso286_limits(
    Query(q): Query<Iso286Query>,
) -> Result<Json<serde_json::Value>, ApiError> {
    if q.class.contains('/') {
        let fit = iso286::resolve_fit(q.nominal, &q.class).map_err(fit_refusal)?;
        return Ok(Json(serde_json::json!({
            "nominal_mm": q.nominal,
            "hole": class_json(q.nominal, &fit.hole),
            "shaft": class_json(q.nominal, &fit.shaft),
            "fit": {
                "kind": fit.kind.label(),
                "min_clearance_mm": fit.min_clearance_mm,
                "max_clearance_mm": fit.max_clearance_mm,
            },
        })));
    }
    let class = iso286::resolve_class(q.nominal, &q.class).map_err(fit_refusal)?;
    let mut body = class_json(q.nominal, &class);
    body["nominal_mm"] = serde_json::json!(q.nominal);
    Ok(Json(body))
}
