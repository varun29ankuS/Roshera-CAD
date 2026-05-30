//! REST fillet payload parsing — variable-radius wire shapes.
//!
//! # Why this module exists
//!
//! The `/api/geometry/fillet` endpoint historically accepted two
//! shapes:
//!
//! - `{ "radius": 2.0 }`       — uniform constant radius for every edge.
//! - `{ "radii":  [1.0, 3.0] }` — per-edge constant radii (parallel to
//!                                `edges`).
//!
//! F3-ε.2 introduces *variable-radius* fillets. The wire shape extends
//! both fields to accept a [`BlendRadiusDto`] in place of a bare number:
//!
//! ```json
//! { "radius": { "kind": "linear", "start": 1.0, "end": 3.0 } }
//! { "radius": { "kind": "variable",
//!               "samples": [[0.0, 1.0], [0.5, 3.0], [1.0, 1.0]] } }
//! { "radii":  [ 2.0,
//!               { "kind": "linear", "start": 1.0, "end": 3.0 } ] }
//! ```
//!
//! [`BlendRadiusDto`]'s manual `Deserialize` already accepts both bare
//! numbers and the tagged object form (see
//! `timeline-engine/src/types.rs`), so legacy clients sending
//! `"radius": 2.0` continue to round-trip unchanged: a bare number
//! deserialises to `BlendRadiusDto::Constant(2.0)` which maps to
//! `FilletType::Constant(2.0)` — byte-identical kernel behaviour.
//!
//! # Why parsing lives in its own module
//!
//! The endpoint handler in `main.rs` is async and tied to
//! `AppState` + `ActiveModel` extractors. Parsing is a pure transform
//! from `serde_json::Value` to `Vec<FilletType>` with no I/O, no
//! locks, no kernel dependencies beyond the `FilletType` enum.
//! Lifting it out means the harness can drive every wire shape
//! through the *real* parser without spinning up the axum router or
//! the geometry kernel.
//!
//! # Validation contract
//!
//! Every numeric value the wire ships through is range-checked at this
//! boundary (CLAUDE.md "External / user input" rule). The kernel
//! re-validates via `validate_fillet_inputs`, but rejecting at the
//! REST edge gives clients a typed `InvalidParameter` ApiError with
//! a precise field path instead of a generic kernel error mid-op.
//!
//! Rules:
//!
//! - Every `Constant(r)`: `r.is_finite() && r > 0`.
//! - Every `Linear { start, end }`: both finite and `> 0`.
//! - Every `Variable(samples)`:
//!   - `samples` non-empty.
//!   - Each `(station, radius)` has `station ∈ [0, 1]`,
//!     `radius.is_finite() && radius > 0`.
//!   - Stations need *not* be monotone — the kernel handles
//!     reordering and de-duplication; we only check ranges here.
//!
//! Mixing `radius` and `radii` in the same request is rejected.
//! Specifying neither is rejected. `radii` length must equal `edges`
//! length.

use crate::error_catalog::{ApiError, ErrorCode};
use geometry_engine::operations::blend_graph::{BlendRadius, EdgeFilletProfile};
use geometry_engine::operations::fillet::FilletType;
use geometry_engine::primitives::edge::EdgeId;
use serde_json::Value;
use std::collections::HashMap;
use timeline_engine::operations::blend_radius_dto_to_fillet_type;

/// Absolute upper bound on any blend-dimension scalar surfaced by the
/// public API (fillet radius, chamfer distance, variable-radius
/// samples, chord lengths). 1e6 in the kernel's working units (mm)
/// is 1 km — well past any reasonable CAD scale and inside the
/// double-precision exact-integer band. The bound is a DoS / sanity
/// gate, not a kernel constraint: the kernel will happily attempt a
/// 1e9 mm radius on a 10 mm box and waste seconds of CPU before
/// returning `RadiusExceedsCurvature`. Rejecting at the boundary
/// turns that into a sub-microsecond 400. AUDIT-H3.
pub const MAX_BLEND_DIMENSION: f64 = 1.0e6;
use timeline_engine::BlendRadiusDto;

/// Parsed + validated per-edge radius profiles, parallel to the
/// `edges` array on the request.
///
/// # Why we hold `BlendRadiusDto`, not `FilletType`
///
/// `FilletType::Function(Box<dyn Fn(f64) -> f64>)` is `!Send + !Sync`,
/// so any future that holds `Vec<FilletType>` across an `.await`
/// fails the axum `Handler` trait bound. The kernel parser path only
/// ever produces three concrete variants (`Constant`, `Linear`,
/// `Variable(samples)`), which `BlendRadiusDto` represents
/// exhaustively — and `BlendRadiusDto` is `Send + Sync` because it's
/// purely owned data. Callers translate via
/// [`to_fillet_type`](FilletRadii::to_fillet_type) inside the
/// model-lock scope, immediately before the kernel call.
#[derive(Debug, Clone)]
pub struct FilletRadii {
    /// One validated `BlendRadiusDto` per edge, parallel to `edges`.
    /// Map to `FilletType` via [`to_fillet_type`](Self::to_fillet_type)
    /// at the kernel call site.
    pub profiles: Vec<BlendRadiusDto>,
    /// Per-edge canonical JSON. Always tagged form
    /// (`{"kind": "constant", "value": …}` etc.), so downstream
    /// consumers parse it identically regardless of whether the
    /// caller sent a bare number or a tagged object.
    pub canonical_per_edge: Vec<Value>,
    /// `true` when every edge resolved to a `Constant(r)` with the
    /// same radius (within `f64::EPSILON`). The endpoint uses this
    /// to take the atomic single-`fillet_edges`-call fast path that
    /// preserves edge-chain grouping for the kernel's blend-
    /// continuity machinery.
    pub uniform_constant: bool,
    /// `true` when every profile is a `Constant(r)`, regardless of
    /// equality. F5-β.5.3 uses this in combination with
    /// `!uniform_constant` to route distinct per-edge constants
    /// through a single atomic [`FilletType::PerEdgeConstant`] kernel
    /// call rather than the per-edge fallback loop — required so the
    /// BlendGraph sees the shared corner as a single 3-edge vertex
    /// and the mixed-radii corner dispatcher (F5-β.3) becomes
    /// reachable from the wire surface.
    pub all_constant: bool,
    /// F5-β.5.9 — per-edge override map keyed by [`EdgeId`]. When
    /// `Some`, the caller supplied the `radius + per_edge_overrides`
    /// wire shape: every edge in the selection picks up its
    /// override profile when present, falling back to the default
    /// profile (broadcast into `profiles`) otherwise. The endpoint
    /// expands this to the kernel's
    /// [`FilletType::PerEdgeProfile`](FilletType::PerEdgeProfile)
    /// shape via [`expand_to_per_edge_profile`](Self::expand_to_per_edge_profile).
    ///
    /// `None` for every legacy wire shape (bare `radius`, `radii`
    /// array), preserving the existing three-arm dispatch.
    pub per_edge_overrides: Option<HashMap<EdgeId, BlendRadiusDto>>,
}

impl FilletRadii {
    /// Translate `profiles[i]` into the kernel's `FilletType`
    /// dispatch shape. Performed at the kernel call site, *inside*
    /// the model-lock scope, so the non-`Send` `FilletType` never
    /// crosses an `.await`.
    pub fn to_fillet_type(&self, i: usize) -> FilletType {
        blend_radius_dto_to_fillet_type(&self.profiles[i])
    }

    /// Build a [`HashMap<EdgeId, f64>`] mapping each edge in `edges`
    /// to its `Constant(r)` profile value, returning `Some` iff every
    /// profile is a `Constant`. Returns `None` when any profile is
    /// `Linear` / `Variable` (caller falls back to the per-edge loop)
    /// or when `edges.len() != self.profiles.len()` (caller bug — the
    /// parser already enforces equal length, so the mismatch path is
    /// defensive).
    ///
    /// The map is the input shape for
    /// [`FilletType::PerEdgeConstant`], which routes a single atomic
    /// `fillet_edges` call carrying distinct per-edge constants. This
    /// is what unblocks the mixed-radii corner dispatcher from the
    /// public wire surface.
    pub fn to_per_edge_constant_map(&self, edges: &[EdgeId]) -> Option<HashMap<EdgeId, f64>> {
        if !self.all_constant || edges.len() != self.profiles.len() {
            return None;
        }
        let mut map = HashMap::with_capacity(edges.len());
        for (&eid, profile) in edges.iter().zip(self.profiles.iter()) {
            match profile {
                BlendRadiusDto::Constant(r) => {
                    map.insert(eid, *r);
                }
                // Defensive — `all_constant` invariant guarantees every
                // profile matches `Constant(_)`, so this arm is
                // unreachable when the parser produced `self`. Treat a
                // mismatch as a contract violation by the caller and
                // surface the lossy outcome (no map) rather than
                // inserting garbage.
                _ => return None,
            }
        }
        Some(map)
    }

    /// F5-β.5.9 — expand the (default `radius`, optional
    /// `per_edge_overrides`) pair into a full per-edge
    /// [`EdgeFilletProfile`] map covering every edge in `edges`.
    /// Edges with an override pick it up; the rest fall back to the
    /// default profile broadcast into `self.profiles[0]`.
    ///
    /// This is the wire-shape expansion documented under
    /// `F5-β.5.9 — Mixed-default DTO ergonomic shape` in the F5-β.5
    /// plan: the kernel always sees the full
    /// [`FilletType::PerEdgeProfile`](FilletType::PerEdgeProfile)
    /// map after expansion, so the per-edge surgery dispatcher
    /// handles the "default + a few overrides" UX flow identically
    /// to an explicit `{edge: profile}` map.
    ///
    /// Returns the expanded map; the caller wraps it in
    /// `FilletType::PerEdgeProfile`. Edges in `self.per_edge_overrides`
    /// that are *not* in `edges` are silently skipped — call
    /// [`validate_overrides_against_edges`](Self::validate_overrides_against_edges)
    /// up-front to reject those at the wire boundary.
    pub fn expand_to_per_edge_profile(
        &self,
        edges: &[EdgeId],
    ) -> HashMap<EdgeId, EdgeFilletProfile> {
        let overrides = self.per_edge_overrides.as_ref();
        // `self.profiles[0]` is the broadcast default (every slot
        // holds the same DTO when the wire shipped a single
        // `radius`). When `profiles` is empty (defensive: caller
        // built `FilletRadii` directly without going through
        // `parse_fillet_radii`), the fallback yields a Constant(0.0)
        // that the kernel rejects at `validate_fillet_inputs`.
        let default_dto = self.profiles.first();
        let mut out: HashMap<EdgeId, EdgeFilletProfile> = HashMap::with_capacity(edges.len());
        for &eid in edges {
            let dto = overrides.and_then(|m| m.get(&eid)).or(default_dto);
            let profile = match dto {
                Some(dto) => blend_radius_dto_to_edge_profile(dto),
                None => EdgeFilletProfile::Radius(BlendRadius::Constant(0.0)),
            };
            out.insert(eid, profile);
        }
        out
    }

    /// F5-β.5.9 — reject `per_edge_overrides` keys that are not
    /// members of the `edges` selection. The parser cannot do this
    /// itself (it doesn't see the parsed `edges` array), so the
    /// endpoint calls this after [`parse_fillet_radii`] and before
    /// the kernel dispatch. Returns `Ok(())` when overrides are
    /// `None` or every key is a member of `edges`.
    pub fn validate_overrides_against_edges(&self, edges: &[EdgeId]) -> Result<(), ApiError> {
        let overrides = match self.per_edge_overrides.as_ref() {
            None => return Ok(()),
            Some(m) => m,
        };
        for &override_eid in overrides.keys() {
            if !edges.iter().any(|&e| e == override_eid) {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!(
                        "'per_edge_overrides' contains edge {override_eid} \
                         that is not in the 'edges' selection"
                    ),
                ));
            }
        }
        Ok(())
    }
}

/// F5-β.5.9 — translate a wire-level [`BlendRadiusDto`] into the
/// kernel's [`EdgeFilletProfile`] shape used inside
/// [`FilletType::PerEdgeProfile`](FilletType::PerEdgeProfile).
///
/// Mirrors timeline-engine's `blend_radius_dto_to_edge_profile`
/// helper at `timeline-engine/src/operations/fillet.rs:404`: the
/// three radius-schedule DTO variants wrap into
/// `EdgeFilletProfile::Radius(BlendRadius::*)`; the `Chord` DTO
/// maps to `EdgeFilletProfile::Chord` so the raw chord length
/// survives intact to surgery time. Duplicated here to keep
/// `fillet_payload` independent of timeline-engine internals —
/// the function is a pure pattern match with no shared state.
fn blend_radius_dto_to_edge_profile(dto: &BlendRadiusDto) -> EdgeFilletProfile {
    match dto {
        BlendRadiusDto::Constant(r) => EdgeFilletProfile::Radius(BlendRadius::Constant(*r)),
        BlendRadiusDto::Linear { start, end } => EdgeFilletProfile::Radius(BlendRadius::Linear {
            start: *start,
            end: *end,
        }),
        BlendRadiusDto::Variable(samples) => {
            EdgeFilletProfile::Radius(BlendRadius::Variable(samples.clone()))
        }
        BlendRadiusDto::Chord(c) => EdgeFilletProfile::Chord(*c),
    }
}

/// Parse the `radius` / `radii` fields of a fillet payload into one
/// `FilletType` per edge.
///
/// `edge_count` is the already-parsed length of the `edges` array —
/// passed in rather than re-derived so the parser has no JSON-key
/// dependencies beyond the radius fields it owns.
pub fn parse_fillet_radii(payload: &Value, edge_count: usize) -> Result<FilletRadii, ApiError> {
    let radius_field = payload.get("radius");
    let radii_field = payload.get("radii");
    let overrides_field = payload.get("per_edge_overrides");

    // F5-β.5.9 — mutual-exclusion gate.
    //
    // Four allowed shapes:
    //   1. `radius`                     → broadcast default
    //   2. `radii`                      → explicit per-edge array
    //   3. `radius + per_edge_overrides`→ default + sparse overrides
    //   4. (nothing)                    → rejected, missing field
    //
    // `radii` + `per_edge_overrides` is rejected — the array shape
    // is itself a full per-edge spec, so combining the two would
    // duplicate the per-edge surface. `per_edge_overrides` without
    // a `radius` default is rejected because there's no fallback
    // profile for edges that don't carry an explicit override.
    if radius_field.is_some() && radii_field.is_some() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "cannot specify both 'radius' and 'radii' — pick one".to_string(),
        ));
    }
    if radii_field.is_some() && overrides_field.is_some() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "cannot specify both 'radii' and 'per_edge_overrides' — \
             'per_edge_overrides' attaches to a single 'radius' default, \
             not a 'radii' array"
                .to_string(),
        ));
    }
    if overrides_field.is_some() && radius_field.is_none() {
        return Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "'per_edge_overrides' requires a default 'radius' for \
             edges without an explicit override"
                .to_string(),
        ));
    }
    if radius_field.is_none() && radii_field.is_none() && overrides_field.is_none() {
        return Err(ApiError::missing_field("radius"));
    }

    let per_edge_overrides = parse_per_edge_overrides(overrides_field)?;

    match (radius_field, radii_field) {
        (Some(_), Some(_)) => unreachable!("mutex gate above rejects this case"),
        (None, None) => unreachable!("missing-field gate above rejects this case"),
        (Some(r), None) => {
            let dto = parse_dto(r, "radius")?;
            let canonical = canonicalise(&dto);
            let uniform_constant = matches!(dto, BlendRadiusDto::Constant(_));
            // Uniform `radius: Constant(r)` populates every slot with
            // the same `Constant` — so `all_constant` is just
            // `uniform_constant` for the single-radius shape.
            let all_constant = uniform_constant;
            Ok(FilletRadii {
                profiles: vec![dto; edge_count],
                canonical_per_edge: vec![canonical; edge_count],
                uniform_constant,
                all_constant,
                per_edge_overrides,
            })
        }
        (None, Some(rs)) => {
            let arr = rs.as_array().ok_or_else(|| {
                ApiError::new(
                    ErrorCode::InvalidParameter,
                    "'radii' must be a JSON array".to_string(),
                )
            })?;
            if arr.len() != edge_count {
                return Err(ApiError::new(
                    ErrorCode::InvalidParameter,
                    format!(
                        "'radii' length {} must equal 'edges' length {}",
                        arr.len(),
                        edge_count
                    ),
                ));
            }
            let mut profiles = Vec::with_capacity(arr.len());
            let mut canonical_per_edge = Vec::with_capacity(arr.len());
            for (i, item) in arr.iter().enumerate() {
                let dto = parse_dto(item, &format!("radii[{i}]"))?;
                canonical_per_edge.push(canonicalise(&dto));
                profiles.push(dto);
            }
            let uniform_constant = match profiles.first() {
                Some(BlendRadiusDto::Constant(r0)) => profiles.iter().all(|p| match p {
                    BlendRadiusDto::Constant(r) => (r - r0).abs() < f64::EPSILON,
                    _ => false,
                }),
                _ => false,
            };
            // `all_constant` ⊇ `uniform_constant`: every profile is a
            // `Constant`, but the values may differ. F5-β.5.3 routes
            // this case through the new `PerEdgeConstant` kernel
            // variant.
            let all_constant = profiles
                .iter()
                .all(|p| matches!(p, BlendRadiusDto::Constant(_)));
            Ok(FilletRadii {
                profiles,
                canonical_per_edge,
                uniform_constant,
                all_constant,
                per_edge_overrides,
            })
        }
    }
}

/// F5-β.5.9 — parse the optional `per_edge_overrides` map from the
/// fillet payload. Returns `None` when the field is absent; returns
/// a validated `HashMap<EdgeId, BlendRadiusDto>` when present.
///
/// Wire shape: `{"7": <BlendRadiusDto>, "12": <BlendRadiusDto>}`.
/// JSON object keys are strings; we parse each key as `EdgeId`
/// (a `u32`). Each value goes through the same `parse_dto` gate
/// as the top-level `radius` field, so every override profile is
/// range-checked at the wire boundary.
///
/// Membership of override keys in the `edges` array is *not*
/// checked here — the parser doesn't see `edges`. The endpoint
/// follows up with
/// [`FilletRadii::validate_overrides_against_edges`].
fn parse_per_edge_overrides(
    field: Option<&Value>,
) -> Result<Option<HashMap<EdgeId, BlendRadiusDto>>, ApiError> {
    let value = match field {
        None => return Ok(None),
        Some(v) => v,
    };
    let obj = value.as_object().ok_or_else(|| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            "'per_edge_overrides' must be a JSON object \
             keyed by stringified EdgeId (u32)"
                .to_string(),
        )
    })?;
    let mut map: HashMap<EdgeId, BlendRadiusDto> = HashMap::with_capacity(obj.len());
    for (key, val) in obj.iter() {
        let edge_id: EdgeId = key.parse().map_err(|_| {
            ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "'per_edge_overrides' key '{key}' is not a valid EdgeId \
                     (expected non-negative integer ≤ u32::MAX)"
                ),
            )
        })?;
        let dto = parse_dto(val, &format!("per_edge_overrides[\"{key}\"]"))?;
        if map.insert(edge_id, dto).is_some() {
            // Two stringified keys collapsed to the same numeric
            // EdgeId — JSON itself would reject duplicate string
            // keys, but `"7"` and `"007"` both parse to `7`. Defend
            // at the boundary so the kernel never sees a silent
            // overwrite.
            return Err(ApiError::new(
                ErrorCode::InvalidParameter,
                format!(
                    "'per_edge_overrides' contains duplicate entries \
                     for EdgeId {edge_id}"
                ),
            ));
        }
    }
    Ok(Some(map))
}

/// Deserialise one radius value (bare number or tagged object) into a
/// `BlendRadiusDto`, then range-check the values.
///
/// `field_path` is woven into the returned `ApiError` so the client
/// learns exactly which array index or field tripped validation
/// (e.g. `radii[2].samples[1].radius`).
fn parse_dto(value: &Value, field_path: &str) -> Result<BlendRadiusDto, ApiError> {
    let dto: BlendRadiusDto = serde_json::from_value(value.clone()).map_err(|e| {
        ApiError::new(
            ErrorCode::InvalidParameter,
            format!("'{field_path}' is not a valid radius profile: {e}"),
        )
    })?;
    validate_dto(&dto, field_path)?;
    Ok(dto)
}

/// Range-check every numeric value the DTO carries.
fn validate_dto(dto: &BlendRadiusDto, field_path: &str) -> Result<(), ApiError> {
    let invalid = |msg: String| ApiError::new(ErrorCode::InvalidParameter, msg);
    // AUDIT-H3: every positive-finite numeric the DTO carries is also
    // gated above by `MAX_BLEND_DIMENSION` to bound CPU spent in the
    // kernel on absurd radii. The check sits next to the positivity
    // gate so the two read as one validation phase.
    let check_scalar = |x: f64, field: &str| -> Result<(), ApiError> {
        if !x.is_finite() || x <= 0.0 {
            return Err(invalid(format!(
                "'{field}' must be a positive finite number, got {x}"
            )));
        }
        if x > MAX_BLEND_DIMENSION {
            return Err(invalid(format!(
                "'{field}'={x} exceeds maximum blend dimension {MAX_BLEND_DIMENSION}"
            )));
        }
        Ok(())
    };
    match dto {
        BlendRadiusDto::Constant(r) => {
            check_scalar(*r, &format!("{field_path}"))?;
        }
        BlendRadiusDto::Linear { start, end } => {
            check_scalar(*start, &format!("{field_path}.start"))?;
            check_scalar(*end, &format!("{field_path}.end"))?;
        }
        BlendRadiusDto::Variable(samples) => {
            if samples.is_empty() {
                return Err(invalid(format!(
                    "'{field_path}.samples' must contain at least one (station, radius) pair"
                )));
            }
            for (i, (station, radius)) in samples.iter().enumerate() {
                if !station.is_finite() || !(0.0..=1.0).contains(station) {
                    return Err(invalid(format!(
                        "'{field_path}.samples[{i}].station' must lie in [0, 1], got {station}"
                    )));
                }
                check_scalar(*radius, &format!("{field_path}.samples[{i}].radius"))?;
            }
        }
        // F5-β.5.7: the `Chord` DTO carries the raw chord length;
        // positivity / finiteness is the same gate the `Constant`
        // arm applies. End-to-end routing of chord profiles through
        // the per-edge `all_simple` payload path is deferred to a
        // follow-up slice; this arm exists so the validator remains
        // exhaustive after the DTO grew a fourth variant.
        BlendRadiusDto::Chord(c) => {
            check_scalar(*c, &format!("{field_path}.chord"))?;
        }
    }
    Ok(())
}

/// Render a `BlendRadiusDto` to its canonical tagged JSON form.
///
/// `serde_json::to_value` round-trips through the DTO's manual
/// `Serialize`, which always emits the tagged shape — so a caller
/// sending bare `2.0` round-trips as
/// `{"kind": "constant", "value": 2.0}` in the broadcast frame.
/// That keeps downstream consumers (timeline mirror, chat) on a
/// single parser path.
fn canonicalise(dto: &BlendRadiusDto) -> Value {
    serde_json::to_value(dto).unwrap_or_else(|_| {
        // BlendRadiusDto's Serialize never returns Err on well-formed
        // input; we already validated the values above. Fall back to
        // null on the impossible path so the endpoint cannot 500 here.
        Value::Null
    })
}

#[cfg(test)]
mod tests {
    //! Wire-shape harness for the F3-ε.2 fillet payload parser.
    //!
    //! Every test exercises the parser at the JSON boundary, with the
    //! same `serde_json::Value` shape a client would POST. Tests are
    //! organised by axis:
    //!
    //! - **backward compatibility** — every legacy `radius` / `radii`
    //!   shape continues to parse and dispatch as it did pre-F3-ε.2.
    //! - **variable-radius positive paths** — `Linear` and `Variable`
    //!   in both `radius` (uniform) and `radii` (per-edge) shapes.
    //! - **negative paths** — every validation error has a dedicated
    //!   pin so wire-shape drift surfaces immediately.
    //! - **canonical round-trip** — bare numbers normalise to tagged
    //!   `{"kind": "constant", …}` form on the way out.
    //! - **uniform_constant flag** — the kernel fast-path indicator
    //!   only fires for all-Constant-and-equal radii.

    use super::*;
    use serde_json::json;

    // The parser holds `BlendRadiusDto` (Send + Sync) rather than the
    // kernel's `FilletType` (whose `Function(Box<dyn Fn>)` variant is
    // `!Send`). Assertions therefore pattern-match on the DTO. The DTO
    // → `FilletType` mapping is exercised end-to-end by
    // `to_fillet_type_matches_dispatch` below — one pin is enough,
    // because the translation lives in `timeline-engine` and has its
    // own unit coverage.

    fn assert_constant(dto: &BlendRadiusDto, want: f64) {
        match dto {
            BlendRadiusDto::Constant(r) => assert!(
                (r - want).abs() < 1e-12,
                "expected Constant({want}), got Constant({r})"
            ),
            other => panic!("expected Constant({want}), got {other:?}"),
        }
    }

    fn assert_variable_linear(dto: &BlendRadiusDto, want_start: f64, want_end: f64) {
        match dto {
            BlendRadiusDto::Linear { start, end } => {
                assert!(
                    (start - want_start).abs() < 1e-12,
                    "start mismatch: {start} vs {want_start}"
                );
                assert!(
                    (end - want_end).abs() < 1e-12,
                    "end mismatch: {end} vs {want_end}"
                );
            }
            other => panic!("expected Linear({want_start}, {want_end}), got {other:?}"),
        }
    }

    fn assert_variable_stations(dto: &BlendRadiusDto, want: &[(f64, f64)]) {
        match dto {
            BlendRadiusDto::Variable(samples) => {
                assert_eq!(samples.len(), want.len(), "sample count mismatch");
                for ((sa, ra), (sw, rw)) in samples.iter().zip(want.iter()) {
                    assert!((sa - sw).abs() < 1e-12, "station mismatch");
                    assert!((ra - rw).abs() < 1e-12, "radius mismatch");
                }
            }
            other => panic!("expected Variable({want:?}), got {other:?}"),
        }
    }

    // --- backward compatibility ------------------------------------

    #[test]
    fn legacy_bare_radius_uniform() {
        let p = json!({ "radius": 2.5 });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert_eq!(r.profiles.len(), 3);
        for dto in &r.profiles {
            assert_constant(dto, 2.5);
        }
        assert!(
            r.uniform_constant,
            "all-equal Constant must set uniform flag"
        );
    }

    #[test]
    fn legacy_bare_radii_per_edge_equal() {
        let p = json!({ "radii": [1.0, 1.0, 1.0] });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert_eq!(r.profiles.len(), 3);
        assert!(r.uniform_constant);
    }

    #[test]
    fn legacy_bare_radii_per_edge_mixed() {
        let p = json!({ "radii": [1.0, 2.0, 3.0] });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert_constant(&r.profiles[0], 1.0);
        assert_constant(&r.profiles[1], 2.0);
        assert_constant(&r.profiles[2], 3.0);
        assert!(
            !r.uniform_constant,
            "differing Constants must not be flagged uniform"
        );
    }

    // --- tagged Constant -------------------------------------------

    #[test]
    fn tagged_constant_uniform() {
        let p = json!({ "radius": { "kind": "constant", "value": 1.5 } });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        for dto in &r.profiles {
            assert_constant(dto, 1.5);
        }
        assert!(r.uniform_constant);
    }

    // --- Linear (legacy two-point variable) ------------------------

    #[test]
    fn linear_radius_uniform() {
        let p = json!({ "radius": { "kind": "linear", "start": 1.0, "end": 3.0 } });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        for dto in &r.profiles {
            assert_variable_linear(dto, 1.0, 3.0);
        }
        assert!(
            !r.uniform_constant,
            "Linear is not a Constant — fast path must not engage"
        );
    }

    #[test]
    fn linear_radii_per_edge() {
        let p = json!({
            "radii": [
                { "kind": "linear", "start": 1.0, "end": 2.0 },
                { "kind": "linear", "start": 2.0, "end": 5.0 }
            ]
        });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        assert_variable_linear(&r.profiles[0], 1.0, 2.0);
        assert_variable_linear(&r.profiles[1], 2.0, 5.0);
        assert!(!r.uniform_constant);
    }

    // --- Variable (per-station) ------------------------------------

    #[test]
    fn variable_radius_uniform() {
        let p = json!({
            "radius": {
                "kind": "variable",
                "samples": [[0.0, 1.0], [0.5, 3.0], [1.0, 1.0]]
            }
        });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        for dto in &r.profiles {
            assert_variable_stations(dto, &[(0.0, 1.0), (0.5, 3.0), (1.0, 1.0)]);
        }
        assert!(!r.uniform_constant);
    }

    #[test]
    fn variable_single_sample_allowed() {
        // One station is degenerate-but-legal — kernel handles it as
        // a constant. Parser must not reject it; the kernel decides
        // whether it's useful.
        let p = json!({
            "radius": { "kind": "variable", "samples": [[0.5, 2.0]] }
        });
        let r = parse_fillet_radii(&p, 1).expect("parse");
        assert_variable_stations(&r.profiles[0], &[(0.5, 2.0)]);
    }

    #[test]
    fn variable_non_monotone_allowed() {
        // Stations don't need to be sorted — `validate_fillet_inputs`
        // in the kernel handles ordering. Parser only checks ranges.
        let p = json!({
            "radius": {
                "kind": "variable",
                "samples": [[0.5, 3.0], [0.0, 1.0], [1.0, 2.0]]
            }
        });
        let r = parse_fillet_radii(&p, 1).expect("parse");
        assert_variable_stations(&r.profiles[0], &[(0.5, 3.0), (0.0, 1.0), (1.0, 2.0)]);
    }

    // --- mixed wire shape in `radii` -------------------------------

    #[test]
    fn radii_mixes_bare_and_tagged() {
        let p = json!({
            "radii": [
                2.0,
                { "kind": "linear", "start": 1.0, "end": 3.0 },
                { "kind": "variable", "samples": [[0.0, 1.0], [1.0, 2.0]] }
            ]
        });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert_constant(&r.profiles[0], 2.0);
        assert_variable_linear(&r.profiles[1], 1.0, 3.0);
        assert_variable_stations(&r.profiles[2], &[(0.0, 1.0), (1.0, 2.0)]);
        assert!(!r.uniform_constant);
    }

    // --- DTO → FilletType translation pin ---------------------------

    #[test]
    fn to_fillet_type_matches_dispatch() {
        // Drive the kernel translation that runs inside the model-lock
        // scope. Each variant must hit the matching `FilletType` arm.
        let p = json!({
            "radii": [
                2.0,
                { "kind": "linear", "start": 1.0, "end": 3.0 },
                { "kind": "variable", "samples": [[0.0, 1.0], [1.0, 2.0]] }
            ]
        });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        match r.to_fillet_type(0) {
            FilletType::Constant(v) => assert!((v - 2.0).abs() < 1e-12),
            other => panic!("expected Constant, got {other:?}"),
        }
        match r.to_fillet_type(1) {
            FilletType::Variable(s, e) => {
                assert!((s - 1.0).abs() < 1e-12);
                assert!((e - 3.0).abs() < 1e-12);
            }
            other => panic!("expected Variable, got {other:?}"),
        }
        match r.to_fillet_type(2) {
            FilletType::VariableStations(samples) => {
                assert_eq!(samples.len(), 2);
                assert!((samples[0].0 - 0.0).abs() < 1e-12);
                assert!((samples[0].1 - 1.0).abs() < 1e-12);
                assert!((samples[1].0 - 1.0).abs() < 1e-12);
                assert!((samples[1].1 - 2.0).abs() < 1e-12);
            }
            other => panic!("expected VariableStations, got {other:?}"),
        }
    }

    // --- canonical round-trip --------------------------------------

    #[test]
    fn canonical_bare_radius_normalises_to_tagged() {
        let p = json!({ "radius": 2.0 });
        let r = parse_fillet_radii(&p, 1).expect("parse");
        // The Serialize impl always emits the tagged form. Anchor on
        // the kind+value pair rather than the whole JSON to keep this
        // test stable across field reorderings.
        let v = &r.canonical_per_edge[0];
        assert_eq!(v["kind"], json!("constant"));
        assert_eq!(v["value"], json!(2.0));
    }

    #[test]
    fn canonical_linear_round_trip() {
        let p = json!({ "radius": { "kind": "linear", "start": 1.0, "end": 3.0 } });
        let r = parse_fillet_radii(&p, 1).expect("parse");
        let v = &r.canonical_per_edge[0];
        assert_eq!(v["kind"], json!("linear"));
        assert_eq!(v["start"], json!(1.0));
        assert_eq!(v["end"], json!(3.0));
    }

    // --- malformed shapes ------------------------------------------

    #[test]
    fn missing_both_fields_rejected() {
        let p = json!({});
        let err = parse_fillet_radii(&p, 1).expect_err("missing radius");
        assert_eq!(err.code, ErrorCode::MissingField);
    }

    #[test]
    fn both_fields_present_rejected() {
        let p = json!({ "radius": 2.0, "radii": [2.0] });
        let err = parse_fillet_radii(&p, 1).expect_err("ambiguous");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn radii_length_mismatch_rejected() {
        let p = json!({ "radii": [1.0, 2.0] });
        let err = parse_fillet_radii(&p, 3).expect_err("length mismatch");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn radii_not_array_rejected() {
        let p = json!({ "radii": "not an array" });
        let err = parse_fillet_radii(&p, 1).expect_err("not array");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn negative_constant_rejected() {
        let p = json!({ "radius": -1.0 });
        let err = parse_fillet_radii(&p, 1).expect_err("negative");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn zero_constant_rejected() {
        let p = json!({ "radius": 0.0 });
        let err = parse_fillet_radii(&p, 1).expect_err("zero");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn non_finite_constant_rejected() {
        let p = json!({ "radius": f64::NAN });
        // NaN serialises to JSON null in serde_json — handled by the
        // DTO deserializer error path.
        let err = parse_fillet_radii(&p, 1).expect_err("NaN");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn linear_negative_start_rejected() {
        let p = json!({ "radius": { "kind": "linear", "start": -1.0, "end": 2.0 } });
        let err = parse_fillet_radii(&p, 1).expect_err("neg start");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn linear_zero_end_rejected() {
        let p = json!({ "radius": { "kind": "linear", "start": 1.0, "end": 0.0 } });
        let err = parse_fillet_radii(&p, 1).expect_err("zero end");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn variable_empty_samples_rejected() {
        let p = json!({ "radius": { "kind": "variable", "samples": [] } });
        let err = parse_fillet_radii(&p, 1).expect_err("empty");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn variable_station_out_of_range_rejected() {
        let p = json!({
            "radius": { "kind": "variable", "samples": [[1.5, 2.0]] }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("station > 1");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn variable_negative_station_rejected() {
        let p = json!({
            "radius": { "kind": "variable", "samples": [[-0.1, 2.0]] }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("station < 0");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn variable_negative_radius_rejected() {
        let p = json!({
            "radius": { "kind": "variable", "samples": [[0.5, -2.0]] }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("negative radius");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn unknown_kind_rejected() {
        let p = json!({ "radius": { "kind": "Bogus", "value": 2.0 } });
        let err = parse_fillet_radii(&p, 1).expect_err("unknown kind");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn linear_missing_start_rejected() {
        let p = json!({ "radius": { "kind": "linear", "end": 2.0 } });
        let err = parse_fillet_radii(&p, 1).expect_err("missing start");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    // --- F5-β.5.3: all_constant flag + per-edge-constant map -------

    #[test]
    fn all_constant_flag_fires_for_uniform_radii() {
        let p = json!({ "radii": [2.0, 2.0, 2.0] });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert!(r.uniform_constant);
        assert!(
            r.all_constant,
            "uniform Constants are necessarily all-Constant"
        );
    }

    #[test]
    fn all_constant_flag_fires_for_distinct_constants() {
        // The headline F5-β.5.3 case — distinct constants per edge.
        // `uniform_constant` is false but `all_constant` is true, so
        // the endpoint routes through the new PerEdgeConstant arm.
        let p = json!({ "radii": [1.0, 1.5, 2.0] });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert!(!r.uniform_constant, "distinct values must not be uniform");
        assert!(
            r.all_constant,
            "every profile is a Constant — must engage the PerEdgeConstant fast path"
        );
    }

    #[test]
    fn all_constant_flag_clears_for_any_variable_profile() {
        // Mixed kinds — one Linear in the array clears `all_constant`
        // so the endpoint falls through to the per-edge loop (which
        // is the only path that can mix Constant/Linear/Variable).
        let p = json!({
            "radii": [
                1.0,
                { "kind": "linear", "start": 1.0, "end": 3.0 },
                2.0
            ]
        });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert!(!r.uniform_constant);
        assert!(
            !r.all_constant,
            "any non-Constant profile must clear all_constant"
        );
    }

    #[test]
    fn to_per_edge_constant_map_builds_full_map_for_distinct_radii() {
        let p = json!({ "radii": [1.0, 1.5, 2.0] });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        // EdgeIds are arbitrary u32 values — the parser does not know
        // them, so the endpoint passes them in.
        let edges: Vec<EdgeId> = vec![7, 9, 12];
        let map = r
            .to_per_edge_constant_map(&edges)
            .expect("all_constant → Some(map)");
        assert_eq!(map.len(), 3);
        assert!((map.get(&7).copied().unwrap_or(0.0) - 1.0).abs() < 1e-12);
        assert!((map.get(&9).copied().unwrap_or(0.0) - 1.5).abs() < 1e-12);
        assert!((map.get(&12).copied().unwrap_or(0.0) - 2.0).abs() < 1e-12);
    }

    #[test]
    fn to_per_edge_constant_map_returns_none_for_mixed_kinds() {
        let p = json!({
            "radii": [
                1.0,
                { "kind": "linear", "start": 1.0, "end": 3.0 }
            ]
        });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        let edges: Vec<EdgeId> = vec![3, 4];
        assert!(
            r.to_per_edge_constant_map(&edges).is_none(),
            "mixed-kind profiles must yield None — caller falls through to per-edge loop"
        );
    }

    #[test]
    fn to_per_edge_constant_map_returns_none_for_length_mismatch() {
        // Defensive: caller passes the wrong number of edges. The
        // parser guarantees `profiles.len() == edge_count` at parse
        // time, so this path triggers only on caller bugs — the
        // map must still come back as `None` rather than silently
        // truncating.
        let p = json!({ "radii": [1.0, 2.0] });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        let edges: Vec<EdgeId> = vec![3]; // length 1, profiles length 2
        assert!(
            r.to_per_edge_constant_map(&edges).is_none(),
            "length mismatch must surface as None"
        );
    }

    #[test]
    fn uniform_radius_field_populates_all_constant() {
        // The single-`radius`-field path must populate both flags
        // consistently with the `radii` array path.
        let p = json!({ "radius": 2.5 });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert!(r.uniform_constant);
        assert!(r.all_constant);
    }

    #[test]
    fn linear_radius_field_clears_all_constant() {
        let p = json!({ "radius": { "kind": "linear", "start": 1.0, "end": 3.0 } });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        assert!(!r.uniform_constant);
        assert!(
            !r.all_constant,
            "Linear radius must clear all_constant in the single-radius shape"
        );
    }

    // --- F5-β.5.9: per_edge_overrides + Mixed{default, overrides} -----
    //
    // Six unit pins matching the plan's test plan section. Each fires
    // on a single, distinct dispatch path through `parse_fillet_radii`
    // (mutex + happy paths) or through `expand_to_per_edge_profile`
    // (expansion shape). End-to-end routing through the live router
    // sits in `router_integration_tests.rs::fillet_default_with_*`.

    #[test]
    fn bare_radius_with_no_overrides_yields_none_overrides() {
        // (1/6) Legacy path: bare `radius` and no overrides leaves
        // `per_edge_overrides == None`, which the endpoint reads as
        // "use the legacy single-profile dispatch". Pins the
        // backward-compat guarantee: existing clients see no
        // behavioural change.
        let p = json!({ "radius": 1.5 });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        assert!(
            r.per_edge_overrides.is_none(),
            "absent overrides field must surface as None"
        );
    }

    #[test]
    fn radius_with_empty_overrides_is_legal_degenerate_shape() {
        // (2/6) `per_edge_overrides: {}` is degenerate but legal —
        // the endpoint expands to the full default for every edge,
        // which is identical to a bare `radius` request. Validates
        // the parser doesn't reject empty maps and that the
        // expansion fills every edge with the default.
        let p = json!({
            "radius": 1.5,
            "per_edge_overrides": {}
        });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        let overrides = r
            .per_edge_overrides
            .as_ref()
            .expect("empty overrides must surface as Some({})");
        assert!(overrides.is_empty(), "empty object must yield empty map");

        // Every edge falls back to the default Constant(1.5).
        let edges: Vec<EdgeId> = vec![10, 20, 30];
        let expanded = r.expand_to_per_edge_profile(&edges);
        assert_eq!(expanded.len(), 3);
        for &eid in &edges {
            assert_eq!(
                expanded.get(&eid),
                Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(1.5))),
                "edge {eid} must fall back to the default"
            );
        }
    }

    #[test]
    fn radius_with_partial_overrides_expands_correctly() {
        // (3/6) Headline F5-β.5.9 shape: default Constant + one
        // explicit Linear override. The expansion picks the override
        // for the named edge and the default for the rest.
        let p = json!({
            "radius": { "kind": "constant", "value": 2.0 },
            "per_edge_overrides": {
                "9": { "kind": "linear", "start": 0.5, "end": 1.5 }
            }
        });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        let overrides = r
            .per_edge_overrides
            .as_ref()
            .expect("partial overrides must surface as Some(map)");
        assert_eq!(overrides.len(), 1);

        let edges: Vec<EdgeId> = vec![7, 9, 12];
        let expanded = r.expand_to_per_edge_profile(&edges);
        assert_eq!(
            expanded.get(&7),
            Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(2.0))),
            "edge 7 must take the default"
        );
        assert_eq!(
            expanded.get(&9),
            Some(&EdgeFilletProfile::Radius(BlendRadius::Linear {
                start: 0.5,
                end: 1.5,
            })),
            "edge 9 must take the Linear override"
        );
        assert_eq!(
            expanded.get(&12),
            Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(2.0))),
            "edge 12 must take the default"
        );
    }

    #[test]
    fn radius_with_full_overrides_uses_overrides_for_every_edge() {
        // (4/6) Degenerate: every edge carries an override. The
        // default never fires. The expanded map is the override map
        // verbatim, regardless of the default's kind.
        let p = json!({
            "radius": { "kind": "constant", "value": 99.0 },
            "per_edge_overrides": {
                "5":  { "kind": "constant", "value": 0.3 },
                "7":  { "kind": "linear", "start": 0.4, "end": 0.6 },
                "12": { "kind": "chord", "value": 0.8 }
            }
        });
        let r = parse_fillet_radii(&p, 3).expect("parse");
        let edges: Vec<EdgeId> = vec![5, 7, 12];
        let expanded = r.expand_to_per_edge_profile(&edges);
        assert_eq!(expanded.len(), 3);
        assert_eq!(
            expanded.get(&5),
            Some(&EdgeFilletProfile::Radius(BlendRadius::Constant(0.3)))
        );
        assert_eq!(
            expanded.get(&7),
            Some(&EdgeFilletProfile::Radius(BlendRadius::Linear {
                start: 0.4,
                end: 0.6
            }))
        );
        assert_eq!(expanded.get(&12), Some(&EdgeFilletProfile::Chord(0.8)));
        // Default never appears.
        for profile in expanded.values() {
            assert_ne!(
                profile,
                &EdgeFilletProfile::Radius(BlendRadius::Constant(99.0)),
                "default must not appear when every edge is overridden"
            );
        }
    }

    #[test]
    fn overrides_without_radius_rejected_at_parse() {
        // (5/6) `per_edge_overrides` without a default `radius` is
        // ambiguous — there's no fallback for unspecified edges.
        // Reject at parse time so the endpoint never sees a half-
        // built map.
        let p = json!({
            "per_edge_overrides": {
                "7": { "kind": "constant", "value": 1.5 }
            }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("overrides-without-default");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn radii_with_overrides_rejected_at_parse() {
        // (6/6) `radii` + `per_edge_overrides` doubles up the per-
        // edge spec. Reject the combination so the wire shape is
        // unambiguous: arrays carry their own per-edge profiles;
        // maps attach to a single default.
        let p = json!({
            "radii": [1.0, 2.0, 3.0],
            "per_edge_overrides": {
                "7": { "kind": "constant", "value": 1.5 }
            }
        });
        let err = parse_fillet_radii(&p, 3).expect_err("radii+overrides");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    // --- F5-β.5.9: defensive parser-shape pins -----------------------
    //
    // Cover edges of the new wire shape that the six headline tests
    // don't pin: non-object overrides, non-integer keys, invalid
    // override DTOs, stray-edge validation. These are not in the
    // plan's six-test list but are required to keep the parser
    // honest at the wire boundary.

    #[test]
    fn overrides_not_object_rejected() {
        let p = json!({
            "radius": 1.0,
            "per_edge_overrides": [1, 2, 3]
        });
        let err = parse_fillet_radii(&p, 1).expect_err("array overrides");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn overrides_with_non_integer_key_rejected() {
        let p = json!({
            "radius": 1.0,
            "per_edge_overrides": {
                "not-a-number": { "kind": "constant", "value": 1.5 }
            }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("bad key");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn overrides_with_invalid_dto_rejected() {
        let p = json!({
            "radius": 1.0,
            "per_edge_overrides": {
                "7": { "kind": "constant", "value": -1.0 }
            }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("negative override");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    #[test]
    fn validate_overrides_against_edges_passes_when_none() {
        let p = json!({ "radius": 1.0 });
        let r = parse_fillet_radii(&p, 1).expect("parse");
        r.validate_overrides_against_edges(&[7])
            .expect("None overrides must pass");
    }

    #[test]
    fn validate_overrides_against_edges_rejects_stray_key() {
        let p = json!({
            "radius": 1.0,
            "per_edge_overrides": {
                "99": { "kind": "constant", "value": 2.0 }
            }
        });
        let r = parse_fillet_radii(&p, 2).expect("parse");
        let err = r
            .validate_overrides_against_edges(&[7, 8])
            .expect_err("stray key 99 must be rejected");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
    }

    // --- AUDIT-H3: MAX_BLEND_DIMENSION upper bound -----------------

    #[test]
    fn constant_radius_above_max_dimension_rejected() {
        let p = json!({ "radius": MAX_BLEND_DIMENSION + 1.0 });
        let err = parse_fillet_radii(&p, 1).expect_err("oversize radius must reject");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
        assert!(
            err.error.contains("exceeds maximum blend dimension"),
            "error must surface the cap; got: {}",
            err.error
        );
    }

    #[test]
    fn linear_radius_start_above_max_dimension_rejected() {
        let p = json!({
            "radius": {
                "kind": "linear",
                "start": MAX_BLEND_DIMENSION * 2.0,
                "end": 1.0
            }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("oversize start must reject");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
        assert!(err.error.contains("start"));
    }

    #[test]
    fn variable_sample_radius_above_max_dimension_rejected() {
        let p = json!({
            "radius": {
                "kind": "variable",
                "samples": [
                    [0.0, 1.0],
                    [0.5, MAX_BLEND_DIMENSION + 1.0],
                    [1.0, 1.0]
                ]
            }
        });
        let err = parse_fillet_radii(&p, 1).expect_err("oversize sample radius must reject");
        assert_eq!(err.code, ErrorCode::InvalidParameter);
        assert!(err.error.contains("samples[1].radius"));
    }

    #[test]
    fn radius_at_exact_max_dimension_accepted() {
        // Boundary test: MAX_BLEND_DIMENSION itself is inclusive
        // (the gate is `x > MAX`, not `x >= MAX`).
        let p = json!({ "radius": MAX_BLEND_DIMENSION });
        let _ = parse_fillet_radii(&p, 1).expect("exact max must parse");
    }
}
