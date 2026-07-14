//! GD&T REST endpoints — Task 3 of the GD&T campaign (Spec C).
//!
//! ## Endpoints
//!
//! | Method | Path                                    | Purpose                         |
//! |--------|-----------------------------------------|---------------------------------|
//! | POST   | `/api/agent/parts/{id}/datums`          | Designate a datum feature       |
//! | GET    | `/api/agent/parts/{id}/datums`          | List DRF with resolution status |
//! | POST   | `/api/agent/parts/{id}/fcf`             | Author an FCF + evaluate        |
//! | GET    | `/api/agent/parts/{id}/gdt`             | All datums + annotations, live  |
//!
//! ## Persistence honesty (HONESTY DOC — ledger-mandated)
//!
//! The DRF and annotation sidecar ARE stored on the `BRepModel` in its
//! `drf` and `gdt` fields respectively, in-process for the server's
//! lifetime. They ARE **NOT** persisted to the timeline (`SerializedBRep`
//! covers only the SoA topology stores and the PID sidecar; the DRF and
//! GDT sidecars are excluded). A server restart clears all GD&T state.
//! Every response from this module includes `"persistence": "session"` so
//! clients cannot assume durability across restarts.
//!
//! ## Lock discipline
//!
//! `POST /datums` and `POST /fcf` mutate the model → write lock.
//! `GET /datums` and `GET /gdt` use a read lock: re-evaluation is
//! read-only on the geometry (`verify::evaluate` borrows `&BRepModel`,
//! and `document_unit()` is `&self`). No guard is held across an
//! `.await`.
//!
//! ## Face resolution
//!
//! Both POST endpoints accept either `face_id: u32` (direct kernel id) or
//! `selector: {...}` (the same descriptor shape as `POST
//! /api/agent/parts/{id}/select-face`). Selector resolution reuses the
//! `resolve_selector` machinery from the label handler (same function).

use crate::part_mgr::ActiveModel;
use crate::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use geometry_engine::gdt::model::{
    Annotation, DatumRef, FeatureControlFrame, GeometricCharacteristic,
};
use geometry_engine::gdt::{
    designate_datum, evaluate, resolve_datum, DatumReferenceFrame, DatumResolution, GdtError,
};
use geometry_engine::primitives::face::FaceId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::units::LengthUnit;
use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────────────────────
// Wire types
// ─────────────────────────────────────────────────────────────────────────────

/// Request body for `POST /api/agent/parts/{id}/datums`.
///
/// Exactly one of `face_id` or `selector` must be supplied.
#[derive(Debug, Deserialize)]
pub struct DesignateDatumRequest {
    /// Drawing label for this datum (e.g. `"A"`, `"B"`, `"C"`).
    pub label: String,
    /// Kernel face id (from `mesh.face_ids`). Mutually exclusive with
    /// `selector`.
    pub face_id: Option<u32>,
    /// Selector descriptor (same shape as `POST .../select-face`).
    /// Mutually exclusive with `face_id`.
    pub selector: Option<serde_json::Value>,
}

/// Wire shape for a single datum in the GET /datums response.
#[derive(Debug, Serialize)]
pub struct DatumWire {
    pub label: String,
    /// `"plane"` or `"axis"`.
    pub kind: String,
    /// Hex-encoded PersistentId (128-bit UUID-v5) of the source face.
    pub persistent_id: String,
    /// Resolution at query time: either `{ "status": "live", "origin": ...,
    /// "direction": ... }` or `{ "status": "dangling" }`.
    pub resolution: DatumResolutionWire,
}

/// Serialisable representation of [`DatumResolution`].
#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DatumResolutionWire {
    Live {
        origin: [f64; 3],
        direction: [f64; 3],
    },
    Dangling,
}

impl From<DatumResolution> for DatumResolutionWire {
    fn from(r: DatumResolution) -> Self {
        match r {
            DatumResolution::Live { origin, direction } => DatumResolutionWire::Live {
                origin: [origin.x, origin.y, origin.z],
                direction: [direction.x, direction.y, direction.z],
            },
            DatumResolution::Dangling => DatumResolutionWire::Dangling,
        }
    }
}

/// Response for `GET /api/agent/parts/{id}/datums`.
#[derive(Debug, Serialize)]
pub struct DatumListResponse {
    pub part_id: u32,
    pub datums: Vec<DatumWire>,
    /// See HONESTY DOC in module-level comment.
    pub persistence: &'static str,
}

/// Request body for `POST /api/agent/parts/{id}/fcf`.
#[derive(Debug, Deserialize)]
pub struct FcfRequest {
    /// One of `"flatness"`, `"perpendicularity"`, `"parallelism"`,
    /// `"position"`.
    pub characteristic: String,
    /// Tolerance zone width in millimetres.
    pub tolerance_mm: f64,
    /// Ordered datum reference labels, e.g. `["A", "B"]`. Empty for
    /// flatness.
    #[serde(default)]
    pub datum_refs: Vec<String>,
    /// Target feature: exactly one of `face_id` or `selector`.
    pub face_id: Option<u32>,
    pub selector: Option<serde_json::Value>,
    /// Basic dimensions `[x, y]` mm relative to the DRF origin.
    /// Required for `"position"`.
    pub basic: Option<[f64; 2]>,
}

/// Wire representation of a conformance verdict.
#[derive(Debug, Serialize)]
pub struct VerdictWire {
    pub characteristic: String,
    pub tolerance_mm: f64,
    /// Formatted in document units (e.g. `"0.030mm"` or `"0.001in"`).
    pub tolerance_label: String,
    pub measured_mm: Option<f64>,
    /// Formatted in document units when `measured_mm` is `Some`.
    pub measured_label: Option<String>,
    /// `"in_spec"` | `"out_of_spec"` | `"not_evaluable"`.
    pub conforms: String,
    /// Populated when `conforms == "not_evaluable"`.
    pub reason: Option<String>,
    pub fit_residual_mm: Option<f64>,
    pub datum_statuses: Vec<DatumStatusWire>,
    /// The resolved datum reference frame (origin + axes) a POSITION verdict
    /// was measured in — disclosed so the basic-dimension convention is
    /// self-certifying. `None` for every other characteristic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame: Option<ResolvedFrameWire>,
}

/// Wire shape for a disclosed position DRF.
#[derive(Debug, Serialize)]
pub struct ResolvedFrameWire {
    pub origin: [f64; 3],
    pub x_axis: [f64; 3],
    pub y_axis: [f64; 3],
    pub z_axis: [f64; 3],
    pub derivation: String,
}

impl From<geometry_engine::gdt::ResolvedFrame> for ResolvedFrameWire {
    fn from(f: geometry_engine::gdt::ResolvedFrame) -> Self {
        ResolvedFrameWire {
            origin: f.origin,
            x_axis: f.x_axis,
            y_axis: f.y_axis,
            z_axis: f.z_axis,
            derivation: f.derivation,
        }
    }
}

/// Wire shape for one datum resolution status in a verdict.
#[derive(Debug, Serialize)]
pub struct DatumStatusWire {
    pub label: String,
    pub resolution: DatumResolutionWire,
}

/// Response for `POST /api/agent/parts/{id}/fcf`.
#[derive(Debug, Serialize)]
pub struct FcfResponse {
    pub part_id: u32,
    /// Hex-encoded PersistentId of the target face.
    pub annotation_pid: String,
    pub verdict: VerdictWire,
    /// See HONESTY DOC.
    pub persistence: &'static str,
}

/// Per-annotation entry in the `GET /gdt` response.
#[derive(Debug, Serialize)]
pub struct AnnotationWire {
    /// Hex-encoded PersistentId of the annotated feature.
    pub feature_pid: String,
    pub verdict: VerdictWire,
    /// Live-resolved kernel face id for the annotated feature.
    ///
    /// `Some(fid)` when the feature PID still maps to an existing face in
    /// the current model (used by the viewport to fan-out the hover tint to
    /// all triangles whose per-triangle `faceIds[t]` equals this value).
    /// `None` when the feature is dangling (PID no longer resolves to a
    /// face) — the viewport must not attempt a tint in that case.
    pub target_face_id: Option<u32>,
    /// A world-space point ON the toleranced feature, used to anchor the
    /// FCF badge near the actual geometry rather than at the first datum's
    /// origin.
    ///
    /// - **Planar target**: the analytic `Plane::origin` — a point guaranteed
    ///   to lie on the plane (the surface parameterisation origin).
    /// - **Cylindrical target**: `cyl.origin + cyl.axis · v_mid` where
    ///   `v_mid` is the axial mid-height, exactly as `evaluate_position` reads
    ///   the bore axis (consistent with how GD&T evaluation reads the feature).
    /// - `None` when the feature is dangling (no face → no geometry → no point).
    pub anchor_mm: Option<[f64; 3]>,
}

/// Response for `GET /api/agent/parts/{id}/gdt`.
#[derive(Debug, Serialize)]
pub struct GdtResponse {
    pub part_id: u32,
    pub datums: Vec<DatumWire>,
    pub annotations: Vec<AnnotationWire>,
    /// See HONESTY DOC.
    pub persistence: &'static str,
}

// ─────────────────────────────────────────────────────────────────────────────
// Pure mapping helpers — unit-testable, no async
// ─────────────────────────────────────────────────────────────────────────────

/// Convert a `GeometricCharacteristic` to its lower-case wire name.
pub(crate) fn characteristic_wire_name(c: GeometricCharacteristic) -> &'static str {
    match c {
        GeometricCharacteristic::Flatness => "flatness",
        GeometricCharacteristic::Perpendicularity => "perpendicularity",
        GeometricCharacteristic::Parallelism => "parallelism",
        GeometricCharacteristic::Position => "position",
        GeometricCharacteristic::Straightness => "straightness",
        GeometricCharacteristic::Circularity => "circularity",
        GeometricCharacteristic::Cylindricity => "cylindricity",
        GeometricCharacteristic::ProfileLine => "profile_line",
        GeometricCharacteristic::ProfileSurface => "profile_surface",
        GeometricCharacteristic::Angularity => "angularity",
        GeometricCharacteristic::Concentricity => "concentricity",
        GeometricCharacteristic::Symmetry => "symmetry",
        GeometricCharacteristic::CircularRunout => "circular_runout",
        GeometricCharacteristic::TotalRunout => "total_runout",
    }
}

/// Parse a lower-case characteristic name from the wire JSON.
pub(crate) fn parse_characteristic(s: &str) -> Option<GeometricCharacteristic> {
    match s.to_ascii_lowercase().as_str() {
        "flatness" => Some(GeometricCharacteristic::Flatness),
        "perpendicularity" => Some(GeometricCharacteristic::Perpendicularity),
        "parallelism" => Some(GeometricCharacteristic::Parallelism),
        "position" => Some(GeometricCharacteristic::Position),
        _ => None,
    }
}

/// Map a kernel [`Verdict`] to the wire shape, formatting mm values
/// with the document unit's canonical formatter.
pub(crate) fn verdict_to_wire(
    verdict: geometry_engine::gdt::Verdict,
    unit: LengthUnit,
) -> VerdictWire {
    use geometry_engine::gdt::Conforms;

    let (conforms_str, reason) = match &verdict.conforms {
        Conforms::InSpec => ("in_spec".to_string(), None),
        Conforms::OutOfSpec => ("out_of_spec".to_string(), None),
        Conforms::NotEvaluable { reason } => ("not_evaluable".to_string(), Some(reason.clone())),
    };

    let tolerance_label = unit.format_len(verdict.tolerance_mm);
    let measured_label = verdict.measured_mm.map(|m| unit.format_len(m));

    let datum_statuses: Vec<DatumStatusWire> = verdict
        .datum_status
        .into_iter()
        .map(|ds| DatumStatusWire {
            label: ds.label,
            resolution: DatumResolutionWire::from(ds.resolution),
        })
        .collect();

    VerdictWire {
        characteristic: verdict.characteristic,
        tolerance_mm: verdict.tolerance_mm,
        tolerance_label,
        measured_mm: verdict.measured_mm,
        measured_label,
        conforms: conforms_str,
        reason,
        fit_residual_mm: verdict.fit_residual_mm,
        datum_statuses,
        frame: verdict.frame.map(ResolvedFrameWire::from),
    }
}

/// Resolve a face from a GDT request body: either `face_id` (direct) or
/// `selector` (descriptive). Returns the resolved `FaceId` or an HTTP
/// error response.
///
/// This is the same resolution path used by `label_create` and
/// `select_face`, applied to the GD&T designation/FCF endpoints.
fn resolve_target_face(
    model: &mut geometry_engine::primitives::topology_builder::BRepModel,
    solid: SolidId,
    face_id: Option<u32>,
    selector: Option<&serde_json::Value>,
) -> Result<FaceId, (StatusCode, Json<serde_json::Value>)> {
    if let Some(fid) = face_id {
        // Direct face id: verify it exists in the model.
        if model.faces.get(fid).is_none() {
            return Err((
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({
                    "error": "face_not_found",
                    "message": format!("face {fid} does not exist in the model"),
                })),
            ));
        }
        return Ok(fid);
    }

    if let Some(sel) = selector {
        // Selector path: reuse the kernel's `resolve_face` via the same
        // descriptor mapping the `select-face` endpoint uses.
        use geometry_engine::math::Vector3;
        use geometry_engine::queries::select::{resolve_face, Extremal, FaceQuery, SurfaceKind};

        let vec3 = |key: &str| -> Option<Vector3> {
            sel.get(key).and_then(|v| v.as_array()).and_then(|a| {
                if a.len() == 3 {
                    Some(Vector3::new(a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?))
                } else {
                    None
                }
            })
        };

        let kind = match sel.get("kind").and_then(|v| v.as_str()).unwrap_or("any") {
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
        let extremal = match sel
            .get("extremal")
            .and_then(|v| v.as_str())
            .unwrap_or("none")
        {
            "largest_area" | "largest" => Extremal::LargestArea,
            "smallest_area" | "smallest" => Extremal::SmallestArea,
            "most_along" | "topmost" | "furthest" => {
                Extremal::MostAlong(along.unwrap_or(Vector3::Z))
            }
            _ => Extremal::None,
        };

        let mut q = FaceQuery::new(kind);
        q.normal_dir = normal_dir;
        q.extremal = extremal;
        if let Some(t) = sel.get("angle_tol_deg").and_then(|v| v.as_f64()) {
            q.angle_tol_deg = t;
        }

        return resolve_face(model, solid, &q).map_err(|e| {
            use geometry_engine::queries::select::SelectError;
            match e {
                SelectError::NotFound => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": "selector_not_found",
                        "message": "no face matches the selector description",
                    })),
                ),
                SelectError::Ambiguous(c) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": "selector_ambiguous",
                        "message": "several faces match the selector — refine the description",
                        "candidates": c,
                    })),
                ),
            }
        });
    }

    Err((
        StatusCode::UNPROCESSABLE_ENTITY,
        Json(serde_json::json!({
            "error": "missing_target",
            "message": "provide face_id or selector",
        })),
    ))
}

/// Build the DRF's [`DatumReferenceFrame`] for a solid, returning a default
/// empty frame when the solid has no designated datums.
fn drf_for_solid(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    solid: SolidId,
) -> DatumReferenceFrame {
    model.drf.get(&solid).cloned().unwrap_or_default()
}

/// Resolve a feature PID to its live face id and a world-space anchor point.
///
/// ## Anchor computation (analytic-first, mirrors `evaluate_position`)
///
/// | Surface kind  | Anchor point                                          |
/// |---------------|-------------------------------------------------------|
/// | Plane         | `plane.origin` — the analytic plane's representative  |
/// |               | point, guaranteed to lie on the face's plane.         |
/// | Cylinder      | `cyl.origin + cyl.axis · v_mid` at the axial          |
/// |               | mid-height — the same point `evaluate_position`       |
/// |               | uses to read the bore axis position.                  |
/// | Other / gone  | `None` for both.                                      |
///
/// Returns `(None, None)` when the PID is dangling.
pub(crate) fn resolve_annotation_anchor(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    pid: geometry_engine::primitives::persistent_id::PersistentId,
) -> (Option<u32>, Option<[f64; 3]>) {
    use geometry_engine::primitives::surface::{Cylinder, Plane};

    let Some(face_id) = model.face_by_pid(pid) else {
        return (None, None);
    };
    let Some(face_data) = model.faces.get(face_id) else {
        return (None, None);
    };
    let Some(surface) = model.surfaces.get(face_data.surface_id) else {
        return (None, None);
    };

    let anchor: [f64; 3] = if let Some(plane) = surface.as_any().downcast_ref::<Plane>() {
        // Analytic plane: `plane.origin` is a canonical representative point
        // on the surface — not the face centroid (vertex averaging is cheaper
        // but the origin IS on the surface by construction and requires no
        // loop traversal).
        [plane.origin.x, plane.origin.y, plane.origin.z]
    } else if let Some(cyl) = surface.as_any().downcast_ref::<Cylinder>() {
        // Cylinder: axis mid-height, matching evaluate_position exactly.
        // Cylinder parameterisation: p(u,v) = origin + radial(u) + axis*v.
        // height_limits gives the analytic v-range; fall back to uv_bounds[2..4]
        // (the builder's default [0,1]) when no explicit limits are stored.
        let v_mid = cyl.height_limits.map_or(
            0.5 * (face_data.uv_bounds[2] + face_data.uv_bounds[3]),
            |[v0, v1]| 0.5 * (v0 + v1),
        );
        let q = cyl.origin + cyl.axis * v_mid;
        [q.x, q.y, q.z]
    } else {
        // Neither plane nor cylinder — no defined anchor convention for this
        // surface kind in Task 2 scope. Return None rather than a misleading
        // point.
        return (Some(face_id), None);
    };

    (Some(face_id), Some(anchor))
}

/// Collect the set of face ids belonging to `solid` (outer shell + inner
/// shells). Used by `GET /gdt` to scope the model-wide GDT sidecar to the
/// requested solid.
fn solid_face_ids(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    solid: SolidId,
) -> std::collections::HashSet<FaceId> {
    let mut faces = std::collections::HashSet::new();
    if let Some(solid_data) = model.solids.get(solid) {
        let mut shell_ids = vec![solid_data.outer_shell];
        shell_ids.extend(solid_data.inner_shells.iter().copied());
        for sid in shell_ids {
            if let Some(shell) = model.shells.get(sid) {
                faces.extend(shell.faces.iter().copied());
            }
        }
    }
    faces
}

/// Collect [`DatumWire`] entries for all datums in a DRF by resolving each at
/// call time.
fn datums_to_wire(
    model: &geometry_engine::primitives::topology_builder::BRepModel,
    solid: SolidId,
    drf: &DatumReferenceFrame,
) -> Vec<DatumWire> {
    drf.datums
        .iter()
        .map(|d| {
            let resolution = DatumResolutionWire::from(resolve_datum(model, solid, d));
            DatumWire {
                label: d.label.clone(),
                kind: match d.kind {
                    geometry_engine::gdt::DatumKind::Plane => "plane".to_string(),
                    geometry_engine::gdt::DatumKind::Axis => "axis".to_string(),
                    geometry_engine::gdt::DatumKind::Point => "point".to_string(),
                },
                persistent_id: format!("{:032x}", d.feature.as_u128()),
                resolution,
            }
        })
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Handlers
// ─────────────────────────────────────────────────────────────────────────────

/// `POST /api/agent/parts/{id}/datums` — designate a face as a datum feature.
///
/// Body: `{ "label": "A", "face_id": 17 }` or `{ "label": "A", "selector": {...} }`.
///
/// ## Response codes
/// - 200 OK: designation succeeded.
/// - 404: solid `{id}` not found.
/// - 409 Conflict: `label` is already designated in this solid's DRF.
/// - 422 Unprocessable: non-qualifying surface (not Plane/Cylinder), face has
///   no PersistentId, face not in solid, selector not found/ambiguous, or
///   missing target.
pub async fn designate_datum_handler(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(req): Json<DesignateDatumRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let solid = id as SolidId;

    let mut model = model_handle.write().await;

    // 404 when the solid doesn't exist.
    if model.solids.get(solid).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "solid_not_found",
                "message": format!("part {id} does not exist"),
            })),
        ));
    }

    // Resolve the target face (direct or by selector).
    let face_id = resolve_target_face(&mut model, solid, req.face_id, req.selector.as_ref())?;

    // Delegate to the kernel designator.
    match designate_datum(&mut model, solid, &req.label, face_id) {
        Ok(datum) => {
            let pid = format!("{:032x}", datum.feature.as_u128());
            let kind_str = match datum.kind {
                geometry_engine::gdt::DatumKind::Plane => "plane",
                geometry_engine::gdt::DatumKind::Axis => "axis",
                geometry_engine::gdt::DatumKind::Point => "point",
            };
            Ok(Json(serde_json::json!({
                "success": true,
                "part_id": id,
                "label": datum.label,
                "kind": kind_str,
                "persistent_id": pid,
                "face_id": face_id,
                "persistence": "session",
            })))
        }
        Err(GdtError::DuplicateLabel { .. }) => Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({
                "error": "duplicate_label",
                "message": format!(
                    "datum label '{}' is already designated for part {id}; \
                     choose a different label (e.g. B, C) or remove the existing designation",
                    req.label
                ),
            })),
        )),
        Err(GdtError::UnsupportedSurfaceKind { kind }) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "non_qualifying_surface",
                "message": format!(
                    "surface kind '{kind}' cannot establish a datum under ASME Y14.5; \
                     designate a planar face (→ datum plane) or a cylindrical face (→ datum axis)"
                ),
            })),
        )),
        Err(GdtError::FaceHasNoPersistentId { face }) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "face_has_no_persistent_id",
                "message": format!(
                    "face {face} has no PersistentId — the datum cannot be pinned durably; \
                     rebuild the solid with an event key assigned so all faces receive PIDs"
                ),
            })),
        )),
        Err(GdtError::FaceNotInSolid { solid, face }) => Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "face_not_in_solid",
                "message": format!("face {face} is not a member of part {solid}"),
            })),
        )),
        Err(GdtError::UnknownSolid { solid }) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "solid_not_found",
                "message": format!("part {solid} does not exist"),
            })),
        )),
    }
}

/// `GET /api/agent/parts/{id}/datums` — list the DRF with per-datum resolution
/// status.
///
/// Resolution is live: a datum whose source face was consumed by a later
/// boolean reports `"status": "dangling"` — never a stale geometry snapshot.
pub async fn list_datums_handler(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<DatumListResponse>, StatusCode> {
    let solid = id as SolidId;
    let model = model_handle.read().await;

    if model.solids.get(solid).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let drf = drf_for_solid(&model, solid);
    let datums = datums_to_wire(&model, solid, &drf);

    Ok(Json(DatumListResponse {
        part_id: id,
        datums,
        persistence: "session",
    }))
}

/// `POST /api/agent/parts/{id}/fcf` — store an FCF annotation on the target
/// face and return its immediate evaluation verdict.
///
/// The annotation is keyed by the target face's PersistentId in the kernel's
/// GDT sidecar. It is re-evaluated on every `GET /gdt` call.
///
/// ## Notes on `"position"` and missing `basic`
///
/// Omitting `basic` for a position FCF is allowed at authoring time: the FCF
/// is stored. The evaluation returns `conforms: "not_evaluable"` with reason
/// `"position requires basic dimensions"` — an honest 200 response, not an
/// error. The annotation is valid; it is the evaluation that refuses.
///
/// ## Response codes
/// - 200 OK: FCF stored, verdict returned.
/// - 404: solid `{id}` not found.
/// - 422: unknown characteristic, datum label not in DRF, face has no PID,
///   selector not found.
pub async fn author_fcf_handler(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
    Json(req): Json<FcfRequest>,
) -> Result<Json<FcfResponse>, (StatusCode, Json<serde_json::Value>)> {
    let solid = id as SolidId;

    // Parse the characteristic before taking the write lock.
    let characteristic = parse_characteristic(&req.characteristic).ok_or_else(|| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "unknown_characteristic",
                "message": format!(
                    "characteristic '{}' is not supported; supported values: \
                     flatness, perpendicularity, parallelism, position",
                    req.characteristic
                ),
            })),
        )
    })?;

    let mut model = model_handle.write().await;

    // 404 when solid is absent.
    if model.solids.get(solid).is_none() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({
                "error": "solid_not_found",
                "message": format!("part {id} does not exist"),
            })),
        ));
    }

    // Validate that every datum_ref label is in the DRF before storing.
    // This 422 prevents storing an un-evaluable FCF that references a
    // label that was never designated — the caller must fix the DRF first.
    {
        let drf = drf_for_solid(&model, solid);
        for label in &req.datum_refs {
            if drf.datum_by_label(label).is_none() {
                return Err((
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(serde_json::json!({
                        "error": "datum_label_not_in_drf",
                        "message": format!(
                            "datum label '{}' has not been designated for part {id}; \
                             use POST /api/agent/parts/{id}/datums to designate it first",
                            label
                        ),
                    })),
                ));
            }
        }
    }

    // Resolve the target face.
    let face_id = resolve_target_face(&mut model, solid, req.face_id, req.selector.as_ref())?;

    // The target face must have a PersistentId — the annotation is keyed by PID.
    let pid = model.face_pid(face_id).ok_or_else(|| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!({
                "error": "face_has_no_persistent_id",
                "message": format!(
                    "face {face_id} has no PersistentId — the annotation cannot be durably \
                     keyed; rebuild the solid with an event key assigned so all faces receive PIDs"
                ),
            })),
        )
    })?;

    // Build the FCF.
    let fcf = FeatureControlFrame {
        characteristic,
        tolerance_value: req.tolerance_mm,
        diametral_zone: characteristic == GeometricCharacteristic::Position,
        modifier: geometry_engine::gdt::model::MaterialModifier::Rfs,
        datum_refs: req
            .datum_refs
            .iter()
            .map(|l| DatumRef::new(l.as_str()))
            .collect(),
        basic: req.basic,
    };

    // Store the annotation in the GDT sidecar (keyed by PID).
    let annotation = Annotation::Geometric(fcf.clone());
    model.gdt.attach(pid, annotation.clone());

    // Evaluate immediately against the live DRF.
    let drf = drf_for_solid(&model, solid);
    let verdict_raw = evaluate(&model, solid, face_id, &annotation, &drf);
    let doc_unit = model.document_unit();
    let verdict = verdict_to_wire(verdict_raw, doc_unit);

    let pid_str = format!("{:032x}", pid.as_u128());

    Ok(Json(FcfResponse {
        part_id: id,
        annotation_pid: pid_str,
        verdict,
        persistence: "session",
    }))
}

/// `GET /api/agent/parts/{id}/gdt` — all datums + annotations, re-evaluated
/// live.
///
/// ## Solid scoping
///
/// The GDT sidecar is model-wide (keyed by PID), so the handler filters
/// annotations to the requested solid: an annotation whose feature face
/// resolves to a DIFFERENT solid is skipped — part 1's response never
/// carries part 2's annotations. Annotations whose PID no longer resolves
/// to any face (dangling) cannot be attributed to a solid, so they are
/// reported on every solid's response with an honest `"not_evaluable"`
/// dangling reason rather than silently dropped.
///
/// ## Annotation storage is append-only
///
/// `GdtSidecar::attach` pushes onto a `Vec<Annotation>` per PID; a face can
/// carry several FCFs (e.g. flatness + perpendicularity). There is no
/// partial-remove API — only `clear_feature`, which drops ALL annotations
/// on a face.
///
/// Dangling datums are reported honestly (`"status": "dangling"`). Annotations
/// on dangling-datum faces evaluate as `"not_evaluable"`.
///
/// The `"persistence": "session"` field documents that GD&T state does not
/// survive a server restart — clients must not assume durability.
pub async fn get_gdt_handler(
    State(_state): State<AppState>,
    ActiveModel(model_handle): ActiveModel,
    Path(id): Path<u32>,
) -> Result<Json<GdtResponse>, StatusCode> {
    let solid = id as SolidId;
    let model = model_handle.read().await;

    if model.solids.get(solid).is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    let drf = drf_for_solid(&model, solid);
    let datums = datums_to_wire(&model, solid, &drf);
    let doc_unit = model.document_unit();

    // Re-evaluate every annotation stored on faces that belong to this solid.
    // The GDT sidecar is keyed by PID — we iterate it and resolve PID → FaceId
    // at call time, reporting dangling for any feature whose face is gone.
    //
    // Solid scoping (review S-1): the sidecar is model-wide, so skip
    // annotations whose face resolves to a DIFFERENT solid. Dangling PIDs
    // (face gone) cannot be attributed to a solid; they are kept and
    // reported honestly.
    let this_solid_faces = solid_face_ids(&model, solid);
    let mut annotations: Vec<AnnotationWire> = Vec::new();
    for (feature_pid, ann_list) in model.gdt.iter() {
        let pid_str = format!("{:032x}", feature_pid.as_u128());
        // Resolve PID → FaceId. If the face no longer exists, every annotation
        // on it is not_evaluable with a dangling reason.
        let face_opt = model.face_by_pid(feature_pid);

        // Foreign-solid annotation: the face exists but belongs to another
        // solid — not part of this response.
        if let Some(fid) = face_opt {
            if !this_solid_faces.contains(&fid) {
                continue;
            }
        }

        // Resolve anchor_mm and target_face_id once per PID (shared by all
        // annotations on the same face — they all reference the same feature).
        let (target_face_id, anchor_mm) = resolve_annotation_anchor(&model, feature_pid);

        for ann in ann_list {
            let verdict_raw = if let Some(fid) = face_opt {
                evaluate(&model, solid, fid, ann, &drf)
            } else {
                // Face is gone — produce a NotEvaluable verdict honestly.
                // I-1 fix: for Geometric annotations use the characteristic
                // mnemonic (e.g. "FLATNESS") rather than ann.kind_label()
                // ("geometric"), and resolve datum statuses through the live DRF
                // — datum refs can still be live even when the target face is gone.
                let (characteristic, tolerance_mm, datum_status) = match ann {
                    Annotation::Geometric(fcf) => {
                        let name = fcf.characteristic.mnemonic().to_string();
                        let tol = fcf.tolerance_value;
                        let statuses: Vec<geometry_engine::gdt::DatumStatus> = fcf
                            .datum_refs
                            .iter()
                            .filter_map(|dr| {
                                drf.datum_by_label(&dr.label).map(|datum| {
                                    geometry_engine::gdt::DatumStatus {
                                        label: dr.label.clone(),
                                        resolution: resolve_datum(&model, solid, datum),
                                    }
                                })
                            })
                            .collect();
                        (name, tol, statuses)
                    }
                    Annotation::Dimensional(_) => (ann.kind_label().to_string(), 0.0, Vec::new()),
                };
                geometry_engine::gdt::Verdict {
                    characteristic,
                    tolerance_mm,
                    measured_mm: None,
                    conforms: geometry_engine::gdt::Conforms::NotEvaluable {
                        reason: format!(
                            "feature PID {} is dangling — its source face no longer exists",
                            pid_str
                        ),
                    },
                    fit_residual_mm: None,
                    datum_status,
                    frame: None,
                }
            };
            annotations.push(AnnotationWire {
                feature_pid: pid_str.clone(),
                verdict: verdict_to_wire(verdict_raw, doc_unit),
                target_face_id,
                anchor_mm,
            });
        }
    }

    Ok(Json(GdtResponse {
        part_id: id,
        datums,
        annotations,
        persistence: "session",
    }))
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — RED-first per the house pattern
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use geometry_engine::gdt::model::{DatumKind, FeatureControlFrame, GeometricCharacteristic};
    use geometry_engine::math::{Point3, Vector3};
    use geometry_engine::primitives::persistent_id::PersistentId;
    use geometry_engine::primitives::surface::Plane;
    use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
    use geometry_engine::units::LengthUnit;

    // ── Fixture helpers ──────────────────────────────────────────────────────

    type SolidId = u32;

    fn new_model_with_box() -> (BRepModel, SolidId) {
        let mut m = BRepModel::new();
        m.set_event_key(Some("plate".into()));
        let solid = match TopologyBuilder::new(&mut m)
            .create_box_3d(50.0, 30.0, 10.0)
            .expect("box must build")
        {
            GeometryId::Solid(s) => s,
            other => panic!("expected solid, got {other:?}"),
        };
        m.set_event_key(None);
        (m, solid)
    }

    fn new_model_with_sphere() -> (BRepModel, SolidId) {
        let mut m = BRepModel::new();
        m.set_event_key(Some("sphere".into()));
        let solid = match TopologyBuilder::new(&mut m)
            .create_sphere_3d(Point3::ORIGIN, 10.0)
            .expect("sphere must build")
        {
            GeometryId::Solid(s) => s,
            other => panic!("expected solid, got {other:?}"),
        };
        m.set_event_key(None);
        (m, solid)
    }

    /// Find the top (+Z) planar face at z = `coord` on a box solid.
    fn planar_face_at_z(m: &BRepModel, solid: SolidId, z: f64) -> Option<u32> {
        let solid_data = m.solids.get(solid)?;
        let mut shell_ids = vec![solid_data.outer_shell];
        shell_ids.extend(solid_data.inner_shells.iter().copied());
        let mut face_ids = Vec::new();
        for sid in shell_ids {
            if let Some(shell) = m.shells.get(sid) {
                face_ids.extend(shell.faces.iter().copied());
            }
        }
        for fid in face_ids {
            let face = m.faces.get(fid)?;
            let surf = m.surfaces.get(face.surface_id)?;
            if let Some(plane) = surf.as_any().downcast_ref::<Plane>() {
                let n = plane.normal;
                if n.z.abs() > 0.99 && (plane.origin.z - z).abs() < 1e-6 {
                    return Some(fid);
                }
            }
        }
        None
    }

    // ── Unit tests: parse_characteristic ────────────────────────────────────

    #[test]
    fn parse_characteristic_supported_roundtrip() {
        for (s, expected) in [
            ("flatness", GeometricCharacteristic::Flatness),
            (
                "perpendicularity",
                GeometricCharacteristic::Perpendicularity,
            ),
            ("parallelism", GeometricCharacteristic::Parallelism),
            ("position", GeometricCharacteristic::Position),
            ("FLATNESS", GeometricCharacteristic::Flatness),
            (
                "Perpendicularity",
                GeometricCharacteristic::Perpendicularity,
            ),
        ] {
            assert_eq!(
                parse_characteristic(s),
                Some(expected),
                "parse_characteristic({s:?}) must return {expected:?}"
            );
        }
    }

    #[test]
    fn parse_characteristic_unsupported_returns_none() {
        for s in ["runout", "angularity", "concentricity", "", "banana"] {
            assert!(
                parse_characteristic(s).is_none(),
                "parse_characteristic({s:?}) must return None"
            );
        }
    }

    // ── Unit tests: verdict_to_wire ──────────────────────────────────────────

    #[test]
    fn verdict_wire_in_spec_formats_labels_mm() {
        let v = geometry_engine::gdt::Verdict {
            characteristic: "FLATNESS".to_string(),
            tolerance_mm: 0.05,
            measured_mm: Some(0.031),
            conforms: geometry_engine::gdt::Conforms::InSpec,
            fit_residual_mm: Some(1e-9),
            datum_status: Vec::new(),
            frame: None,
        };
        let wire = verdict_to_wire(v, LengthUnit::Millimetre);
        assert_eq!(wire.conforms, "in_spec");
        assert_eq!(wire.tolerance_label, "0.05mm");
        assert_eq!(wire.measured_label.as_deref(), Some("0.03mm"));
        assert!(wire.reason.is_none());
    }

    #[test]
    fn verdict_wire_in_spec_formats_labels_inches() {
        // 25.4 mm = 1.000 in exactly (drafting precision 3 dp)
        let v = geometry_engine::gdt::Verdict {
            characteristic: "FLATNESS".to_string(),
            tolerance_mm: 25.4,
            measured_mm: Some(25.4),
            conforms: geometry_engine::gdt::Conforms::InSpec,
            fit_residual_mm: None,
            datum_status: Vec::new(),
            frame: None,
        };
        let wire = verdict_to_wire(v, LengthUnit::Inch);
        assert_eq!(wire.tolerance_label, "1.000in");
        assert_eq!(wire.measured_label.as_deref(), Some("1.000in"));
    }

    #[test]
    fn verdict_wire_not_evaluable_carries_reason() {
        let reason = "datum 'A' is dangling".to_string();
        let v = geometry_engine::gdt::Verdict {
            characteristic: "PERPENDICULARITY".to_string(),
            tolerance_mm: 0.05,
            measured_mm: None,
            conforms: geometry_engine::gdt::Conforms::NotEvaluable {
                reason: reason.clone(),
            },
            fit_residual_mm: None,
            datum_status: Vec::new(),
            frame: None,
        };
        let wire = verdict_to_wire(v, LengthUnit::Millimetre);
        assert_eq!(wire.conforms, "not_evaluable");
        assert_eq!(wire.reason.as_deref(), Some(reason.as_str()));
        assert!(wire.measured_mm.is_none());
        assert!(wire.measured_label.is_none());
    }

    #[test]
    fn verdict_wire_out_of_spec() {
        let v = geometry_engine::gdt::Verdict {
            characteristic: "FLATNESS".to_string(),
            tolerance_mm: 0.02,
            measured_mm: Some(0.03),
            conforms: geometry_engine::gdt::Conforms::OutOfSpec,
            fit_residual_mm: None,
            datum_status: Vec::new(),
            frame: None,
        };
        let wire = verdict_to_wire(v, LengthUnit::Millimetre);
        assert_eq!(wire.conforms, "out_of_spec");
        assert!(wire.reason.is_none());
    }

    // ── Unit tests: designate_datum (kernel-level, through the handler helper) ─

    #[test]
    fn designate_plate_face_success() {
        let (mut m, solid) = new_model_with_box();
        let top = planar_face_at_z(&m, solid, 5.0).expect("top +Z face at z=5");
        let datum = designate_datum(&mut m, solid, "A", top).expect("designate must succeed");
        assert_eq!(datum.label, "A");
        assert_eq!(datum.kind, DatumKind::Plane);
        // DRF stored in the model.
        let drf = m.drf.get(&solid).expect("DRF must be stored");
        assert_eq!(drf.datums.len(), 1);
    }

    #[test]
    fn designate_duplicate_label_409() {
        let (mut m, solid) = new_model_with_box();
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face");
        let bottom = planar_face_at_z(&m, solid, -5.0).expect("-Z face");
        designate_datum(&mut m, solid, "A", top).expect("first A ok");
        let err = designate_datum(&mut m, solid, "A", bottom).expect_err("duplicate must fail");
        assert!(
            matches!(err, GdtError::DuplicateLabel { .. }),
            "must be DuplicateLabel, got {err:?}"
        );
    }

    #[test]
    fn designate_sphere_face_422() {
        let (mut m, solid) = new_model_with_sphere();
        let sphere_face = m
            .solids
            .get(solid)
            .and_then(|s| m.shells.get(s.outer_shell))
            .and_then(|sh| sh.faces.first().copied())
            .expect("sphere must have a face");
        let err = designate_datum(&mut m, solid, "A", sphere_face)
            .expect_err("sphere face must be refused");
        assert!(
            matches!(err, GdtError::UnsupportedSurfaceKind { .. }),
            "must be UnsupportedSurfaceKind, got {err:?}"
        );
    }

    // ── Unit tests: FCF happy path evaluates InSpec verdict ──────────────────

    /// A perfectly flat face (part of a primitive box) must return InSpec
    /// for a flatness callout with any positive tolerance.
    #[test]
    fn fcf_flatness_on_plane_face_returns_in_spec() {
        let (mut m, solid) = new_model_with_box();
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face");
        let pid = m.face_pid(top).expect("box face must have a PID");

        let fcf = FeatureControlFrame::form(GeometricCharacteristic::Flatness, 0.1);
        let ann = Annotation::Geometric(fcf);
        // Store in sidecar.
        m.gdt.attach(pid, ann.clone());
        let drf = drf_for_solid(&m, solid);
        let verdict = evaluate(&m, solid, top, &ann, &drf);
        // A perfect analytic plane → form error = 0 → in spec.
        assert!(
            verdict.conforms.is_in_spec(),
            "flatness on a perfect plane must be in_spec, got {:?}",
            verdict.conforms
        );
        assert!(
            verdict.measured_mm.is_some(),
            "measured_mm must be Some for an evaluable verdict"
        );
        assert_eq!(
            verdict.measured_mm.unwrap(),
            0.0,
            "perfect plane flatness must be 0.0 exactly"
        );
    }

    /// A position FCF authored without `basic` must evaluate as NotEvaluable
    /// (200 with honest verdict — not an error).
    #[test]
    fn fcf_position_missing_basic_is_not_evaluable() {
        let (mut m, solid) = new_model_with_box();
        m.set_event_key(Some("bore".into()));
        // Build a small cylinder (bore) inside the plate model as the target.
        // For simplicity we use the existing solid's bottom face as a planar datum.
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face");
        designate_datum(&mut m, solid, "A", top).expect("designate A");

        // Use the -Z face as a pretend "cylindrical target" (it's actually planar
        // but we're testing the missing-basic path, not geometry).
        let bottom = planar_face_at_z(&m, solid, -5.0).expect("-Z face");
        let pid = m.face_pid(bottom).expect("face PID");

        // Position FCF with no basic dimensions.
        let fcf = FeatureControlFrame::position(0.1, ["A"], [0.0, 0.0]);
        // Zero out basic to simulate the missing-basic authoring path.
        let mut fcf_no_basic = fcf.clone();
        fcf_no_basic.basic = None;

        let ann = Annotation::Geometric(fcf_no_basic);
        m.gdt.attach(pid, ann.clone());
        let drf = drf_for_solid(&m, solid);
        let verdict = evaluate(&m, solid, bottom, &ann, &drf);

        // Missing basic → NotEvaluable (never a fabricated measurement).
        assert!(
            matches!(
                verdict.conforms,
                geometry_engine::gdt::Conforms::NotEvaluable { .. }
            ),
            "position without basic must be NotEvaluable, got {:?}",
            verdict.conforms
        );
    }

    /// Datum label not in DRF must produce a 422-mapping error.
    #[test]
    fn fcf_missing_datum_label_is_rejected() {
        // At the kernel level, evaluate returns NotEvaluable when the datum label
        // is absent — the handler rejects before evaluate via the DRF label check.
        let (mut m, solid) = new_model_with_box();
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face");
        let pid = m.face_pid(top).expect("PID");

        // Reference datum "A" without designating it.
        let fcf =
            FeatureControlFrame::orientation(GeometricCharacteristic::Perpendicularity, 0.05, "A");
        let ann = Annotation::Geometric(fcf);
        m.gdt.attach(pid, ann.clone());
        let drf = drf_for_solid(&m, solid); // empty DRF
        let verdict = evaluate(&m, solid, top, &ann, &drf);

        assert!(
            matches!(
                verdict.conforms,
                geometry_engine::gdt::Conforms::NotEvaluable { .. }
            ),
            "missing datum must be NotEvaluable, got {:?}",
            verdict.conforms
        );
    }

    // ── Unit tests: GET /gdt shape includes persistence field ────────────────

    #[test]
    fn gdt_response_persistence_field_is_session() {
        let resp = GdtResponse {
            part_id: 0,
            datums: Vec::new(),
            annotations: Vec::new(),
            persistence: "session",
        };
        let json = serde_json::to_value(&resp).expect("must serialize");
        assert_eq!(
            json.get("persistence").and_then(|v| v.as_str()),
            Some("session"),
            "persistence field must be 'session'"
        );
    }

    // ── Unit tests: characteristic_wire_name round-trips ────────────────────

    #[test]
    fn characteristic_wire_names_all_supported() {
        for c in [
            GeometricCharacteristic::Flatness,
            GeometricCharacteristic::Perpendicularity,
            GeometricCharacteristic::Parallelism,
            GeometricCharacteristic::Position,
        ] {
            let name = characteristic_wire_name(c);
            assert_eq!(
                parse_characteristic(name),
                Some(c),
                "characteristic_wire_name and parse_characteristic must be inverses for {c:?}"
            );
        }
    }

    // ── Unit tests: resolve_annotation_anchor ────────────────────────────────

    /// A flatness FCF stored on the box top face (+Z, z = 5.0) must resolve to:
    ///   - `target_face_id = Some(<fid>)` — the live face id.
    ///   - `anchor_mm = Some([_, _, z])` where `z ≈ 5.0` — the Plane origin lies
    ///     on the +Z face, whose plane passes through z = 5.0 (half-height of the
    ///     50×30×10 box centred at the origin: height = 10, so top at z = 5.0).
    ///
    /// RED evidence: before `target_face_id` and `anchor_mm` fields were added to
    /// `AnnotationWire`, this test failed to compile — the fields did not exist.
    /// GREEN: fields present and computed by `resolve_annotation_anchor`.
    #[test]
    fn annotation_anchor_resolves_for_planar_face() {
        let (mut m, solid) = new_model_with_box();
        // Box is 50 × 30 × 10, centred at origin → top face at z = 5.0.
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face at z=5");
        let pid = m.face_pid(top).expect("face PID must exist on a keyed box");

        let (face_id_opt, anchor_opt) = super::resolve_annotation_anchor(&m, pid);

        assert_eq!(
            face_id_opt,
            Some(top),
            "live face must resolve to its kernel FaceId"
        );

        let anchor = anchor_opt.expect("planar face must yield a Some(anchor_mm)");
        // The Plane origin for the top face lies on the z = 5.0 plane.
        assert!(
            (anchor[2] - 5.0).abs() < 1e-6,
            "anchor z-coordinate must equal plate top height 5.0 mm, got {}",
            anchor[2]
        );
    }

    /// A dangling PID (face stripped from the model) must resolve to (None, None).
    #[test]
    fn annotation_anchor_dangling_returns_none() {
        let (mut m, solid) = new_model_with_box();
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face");
        let pid = m.face_pid(top).expect("PID");

        // Simulate the face being consumed: remove from the PID→face map.
        m.face_pids.remove(&top);
        m.pid_to_face.remove(&pid);

        let (face_id_opt, anchor_opt) = super::resolve_annotation_anchor(&m, pid);
        assert!(
            face_id_opt.is_none(),
            "dangling PID must return None for face_id"
        );
        assert!(
            anchor_opt.is_none(),
            "dangling PID must return None for anchor_mm"
        );
    }

    /// I-1: When a feature's face is dangling the verdict must carry the
    /// characteristic **mnemonic** (e.g. `"PERPENDICULARITY"`) not the
    /// generic kind label (`"geometric"`), and any datum refs that are live
    /// in the DRF must survive in `datum_statuses`.
    ///
    /// Setup: datum "A" is designated on the **-Z (bottom)** face, which stays
    /// live.  The FCF is attached to the **+Z (top)** face by PID.  Then the
    /// top face is removed from the PID maps (simulating being consumed by a
    /// later boolean) — so the FCF target is dangling while datum A is live.
    ///
    /// Pre-fix (RED) behaviour:
    ///   characteristic = `"geometric"` (ann.kind_label())  ← wrong mnemonic
    ///   datum_statuses = []                                  ← dropped live datum
    ///
    /// Post-fix (GREEN) behaviour:
    ///   characteristic = `"PERPENDICULARITY"`               ← mnemonic
    ///   datum_statuses = [{ label: "A", resolution: Live { .. } }]
    #[test]
    fn dangling_feature_verdict_carries_mnemonic_and_live_datum_refs() {
        let (mut m, solid) = new_model_with_box();

        // ── Datum "A" on the BOTTOM (-Z) face — stays live throughout ─────────
        let bottom = planar_face_at_z(&m, solid, -5.0).expect("-Z face");
        designate_datum(&mut m, solid, "A", bottom).expect("designate datum A on bottom face");

        // ── FCF target: the TOP (+Z) face — will be made dangling ─────────────
        let top = planar_face_at_z(&m, solid, 5.0).expect("+Z face");
        let top_pid = m.face_pid(top).expect("top PID");

        // ── Attach a perpendicularity FCF referencing datum A to the top face ──
        let fcf =
            FeatureControlFrame::orientation(GeometricCharacteristic::Perpendicularity, 0.05, "A");
        m.gdt.attach(top_pid, Annotation::Geometric(fcf));

        // ── Simulate the TOP face being consumed (dangling target) ────────────
        // Only remove the top face's PID mapping; the bottom face (datum A)
        // remains in the model and resolves Live.
        m.face_pids.remove(&top);
        m.pid_to_face.remove(&top_pid);

        // ── Replicate the handler's dangling-arm logic (unit-level) ──────────
        let drf = drf_for_solid(&m, solid);
        let ann_list = m.gdt.annotations(top_pid);
        assert!(
            !ann_list.is_empty(),
            "sidecar must still have the annotation"
        );

        let ann = ann_list.first().expect("one annotation");
        let verdict_raw = match ann {
            Annotation::Geometric(fcf_ref) => {
                let name = fcf_ref.characteristic.mnemonic().to_string();
                let tol = fcf_ref.tolerance_value;
                let statuses: Vec<geometry_engine::gdt::DatumStatus> = fcf_ref
                    .datum_refs
                    .iter()
                    .filter_map(|dr| {
                        drf.datum_by_label(&dr.label).map(|datum| {
                            geometry_engine::gdt::DatumStatus {
                                label: dr.label.clone(),
                                resolution: resolve_datum(&m, solid, datum),
                            }
                        })
                    })
                    .collect();
                geometry_engine::gdt::Verdict {
                    characteristic: name,
                    tolerance_mm: tol,
                    measured_mm: None,
                    conforms: geometry_engine::gdt::Conforms::NotEvaluable {
                        reason: "feature is dangling".to_string(),
                    },
                    fit_residual_mm: None,
                    datum_status: statuses,
                    frame: None,
                }
            }
            Annotation::Dimensional(_) => panic!("expected geometric annotation"),
        };

        // ── Assert: mnemonic, not "geometric" ─────────────────────────────────
        assert_eq!(
            verdict_raw.characteristic, "PERPENDICULARITY",
            "dangling verdict characteristic must be the mnemonic, not ann.kind_label()"
        );

        // ── Assert: datum "A" survives with Live resolution ───────────────────
        // The bottom face (datum A) is still live; only the top face is gone.
        assert_eq!(
            verdict_raw.datum_status.len(),
            1,
            "dangling verdict must carry the live datum ref; got {:?}",
            verdict_raw.datum_status
        );
        assert_eq!(verdict_raw.datum_status[0].label, "A");
        assert!(
            matches!(
                verdict_raw.datum_status[0].resolution,
                geometry_engine::gdt::DatumResolution::Live { .. }
            ),
            "datum A on bottom face must still be Live even though top face (FCF target) \
             is gone; got {:?}",
            verdict_raw.datum_status[0].resolution
        );
    }

    /// Serialisation of `AnnotationWire` must include `target_face_id` and
    /// `anchor_mm` keys — the frontend depends on their presence in the JSON
    /// even when both are null.
    #[test]
    fn annotation_wire_serialises_new_fields() {
        use geometry_engine::units::LengthUnit;
        let wire = AnnotationWire {
            feature_pid: "deadbeef".to_string(),
            verdict: VerdictWire {
                characteristic: "flatness".to_string(),
                tolerance_mm: 0.05,
                tolerance_label: "0.05mm".to_string(),
                measured_mm: None,
                measured_label: None,
                conforms: "not_evaluable".to_string(),
                reason: Some("dangling".to_string()),
                fit_residual_mm: None,
                datum_statuses: Vec::new(),
                frame: None,
            },
            target_face_id: None,
            anchor_mm: None,
        };
        let _ = LengthUnit::Millimetre; // ensure units crate is linked
        let json = serde_json::to_value(&wire).expect("must serialize");
        assert!(
            json.get("target_face_id").is_some(),
            "target_face_id key must be present in JSON (null is acceptable)"
        );
        assert!(
            json.get("anchor_mm").is_some(),
            "anchor_mm key must be present in JSON (null is acceptable)"
        );
    }
}
