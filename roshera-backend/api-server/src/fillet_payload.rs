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
use geometry_engine::operations::fillet::FilletType;
use serde_json::Value;
use timeline_engine::operations::blend_radius_dto_to_fillet_type;
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
}

impl FilletRadii {
    /// Translate `profiles[i]` into the kernel's `FilletType`
    /// dispatch shape. Performed at the kernel call site, *inside*
    /// the model-lock scope, so the non-`Send` `FilletType` never
    /// crosses an `.await`.
    pub fn to_fillet_type(&self, i: usize) -> FilletType {
        blend_radius_dto_to_fillet_type(&self.profiles[i])
    }
}

/// Parse the `radius` / `radii` fields of a fillet payload into one
/// `FilletType` per edge.
///
/// `edge_count` is the already-parsed length of the `edges` array —
/// passed in rather than re-derived so the parser has no JSON-key
/// dependencies beyond the radius fields it owns.
pub fn parse_fillet_radii(
    payload: &Value,
    edge_count: usize,
) -> Result<FilletRadii, ApiError> {
    let radius_field = payload.get("radius");
    let radii_field = payload.get("radii");

    match (radius_field, radii_field) {
        (Some(_), Some(_)) => Err(ApiError::new(
            ErrorCode::InvalidParameter,
            "cannot specify both 'radius' and 'radii' — pick one".to_string(),
        )),
        (None, None) => Err(ApiError::missing_field("radius")),
        (Some(r), None) => {
            let dto = parse_dto(r, "radius")?;
            let canonical = canonicalise(&dto);
            let uniform_constant = matches!(dto, BlendRadiusDto::Constant(_));
            Ok(FilletRadii {
                profiles: vec![dto; edge_count],
                canonical_per_edge: vec![canonical; edge_count],
                uniform_constant,
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
            Ok(FilletRadii {
                profiles,
                canonical_per_edge,
                uniform_constant,
            })
        }
    }
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
    match dto {
        BlendRadiusDto::Constant(r) => {
            if !r.is_finite() || *r <= 0.0 {
                return Err(invalid(format!(
                    "'{field_path}' radius must be a positive finite number, got {r}"
                )));
            }
        }
        BlendRadiusDto::Linear { start, end } => {
            if !start.is_finite() || *start <= 0.0 {
                return Err(invalid(format!(
                    "'{field_path}.start' must be a positive finite number, got {start}"
                )));
            }
            if !end.is_finite() || *end <= 0.0 {
                return Err(invalid(format!(
                    "'{field_path}.end' must be a positive finite number, got {end}"
                )));
            }
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
                if !radius.is_finite() || *radius <= 0.0 {
                    return Err(invalid(format!(
                        "'{field_path}.samples[{i}].radius' must be a positive finite number, got {radius}"
                    )));
                }
            }
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
        assert!(r.uniform_constant, "all-equal Constant must set uniform flag");
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
}
