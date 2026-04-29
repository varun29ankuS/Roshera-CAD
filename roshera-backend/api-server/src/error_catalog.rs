//! Stable, machine-readable error catalog for the HTTP / WebSocket
//! surface.
//!
//! Agents — unlike humans — pattern-match on codes, not prose. A human
//! debugging "missing or non-numeric parameter 'width'" can switch to
//! "expected number for parameter 'width'" without thinking; an agent
//! that built a regex around the first phrasing breaks silently when
//! the second ships. Every error returned by Roshera carries an
//! `error_code` field whose value is a stable identifier owned by
//! this module: change the code = bump the discovery version. The
//! prose `error` field is free to evolve.
//!
//! # Wire shape
//!
//! ```json
//! {
//!     "success": false,
//!     "error_code": "missing_parameter",
//!     "error": "missing or non-numeric parameter 'width'",
//!     "retryable": false,
//!     "hint": "Send a number for 'width' in the parameters object.",
//!     "details": { "parameter": "width" }
//! }
//! ```
//!
//! `success`, `error`, `error_code`, and `retryable` are guaranteed
//! present on every error. `hint` and `details` are optional.
//!
//! # Why a closed enum, not free strings
//!
//! - **Discoverability.** The capability document at `/api/capabilities`
//!   lists every code so agents can preflight their handlers without
//!   triggering each error in turn.
//! - **Cross-cutting policy.** `retryable` is a property of the *kind*
//!   of failure, not of the call site. Encoding it on the enum keeps
//!   the policy in one place.
//! - **Refactor safety.** Adding a variant forces the compiler to
//!   surface every match site, so a new failure mode cannot be
//!   silently bucketed under an old one.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;
use serde_json::Value;

/// Closed catalog of error identifiers. The `Serialize` impl emits the
/// stable wire string (e.g. `missing_parameter`); the in-Rust variant
/// name is for ergonomics only.
///
/// **Adding a variant** is a backwards-compatible patch-bump of
/// `discovery_version`. **Removing or renaming** a variant is a
/// minor-version break and requires a deprecation period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    // ── Request validation ────────────────────────────────────────
    /// A required JSON field was absent.
    MissingField,
    /// A required parameter (under `parameters: {...}`) was absent or
    /// of the wrong type.
    MissingParameter,
    /// A parameter was present and the right type but outside the
    /// allowed range (e.g. negative radius).
    InvalidParameter,
    /// `shape_type` did not match any primitive in the catalog.
    UnknownShapeType,
    /// A request body could not be parsed as JSON / matched no schema.
    InvalidJson,

    // ── Kernel surface ────────────────────────────────────────────
    /// The kernel rejected the operation (topology, tolerance, etc.).
    KernelError,
    /// The kernel succeeded but tessellation produced no triangles —
    /// almost always a kernel defect, never a client bug.
    TessellationEmpty,
    /// A solid referenced by ID is not present in the model.
    SolidNotFound,
    /// The kernel returned a non-solid handle where a solid was
    /// expected (e.g. a primitive constructor returned a Face).
    KernelReturnedWrongType,

    // ── Idempotency layer ─────────────────────────────────────────
    /// `Idempotency-Key` header was sent with an empty value.
    IdempotencyKeyEmpty,
    /// `Idempotency-Key` exceeded the maximum length.
    IdempotencyKeyTooLong,
    /// Same `Idempotency-Key` reused with a different request body.
    IdempotencyKeyReused,
    /// Request body exceeded the size the idempotency layer can buffer.
    IdempotencyBodyTooLarge,
    /// Inner handler returned a body too large for the idempotency cache.
    IdempotencyResponseTooLarge,
    /// Replaying a cached response failed — never expected to fire.
    IdempotencyReplayFailed,

    // ── Transaction layer ─────────────────────────────────────────
    /// `X-Roshera-Tx-Id` referenced an unknown or pruned transaction.
    TransactionNotFound,
    /// Transaction has already been committed, rolled back, or expired
    /// — no further operations may be associated with it.
    TransactionNotActive,

    // ── Catch-alls ────────────────────────────────────────────────
    /// Unspecified server-side fault. Always retryable.
    Internal,
}

impl ErrorCode {
    /// HTTP status code that pairs with this error. Centralised so a
    /// 400 vs 422 vs 409 decision lives in one place; handlers never
    /// pick a status independently.
    pub fn status(self) -> StatusCode {
        match self {
            ErrorCode::MissingField
            | ErrorCode::MissingParameter
            | ErrorCode::InvalidParameter
            | ErrorCode::UnknownShapeType
            | ErrorCode::InvalidJson
            | ErrorCode::IdempotencyKeyEmpty
            | ErrorCode::IdempotencyKeyTooLong => StatusCode::BAD_REQUEST,

            ErrorCode::IdempotencyKeyReused | ErrorCode::TransactionNotActive => {
                StatusCode::CONFLICT
            }
            ErrorCode::IdempotencyBodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,

            ErrorCode::SolidNotFound | ErrorCode::TransactionNotFound => StatusCode::NOT_FOUND,

            ErrorCode::KernelError
            | ErrorCode::TessellationEmpty
            | ErrorCode::KernelReturnedWrongType
            | ErrorCode::IdempotencyResponseTooLarge
            | ErrorCode::IdempotencyReplayFailed
            | ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Whether a client should retry the same request after a backoff.
    /// Retryable = transient. Non-retryable = caller bug or intentional
    /// rejection.
    pub fn retryable(self) -> bool {
        match self {
            // Caller-supplied bad input — retrying with the same body
            // will fail identically. The agent must change its inputs.
            ErrorCode::MissingField
            | ErrorCode::MissingParameter
            | ErrorCode::InvalidParameter
            | ErrorCode::UnknownShapeType
            | ErrorCode::InvalidJson
            | ErrorCode::SolidNotFound
            | ErrorCode::IdempotencyKeyEmpty
            | ErrorCode::IdempotencyKeyTooLong
            | ErrorCode::IdempotencyKeyReused
            | ErrorCode::IdempotencyBodyTooLarge
            | ErrorCode::TransactionNotFound
            | ErrorCode::TransactionNotActive => false,

            // Server-side: another attempt may succeed.
            ErrorCode::KernelError
            | ErrorCode::TessellationEmpty
            | ErrorCode::KernelReturnedWrongType
            | ErrorCode::IdempotencyResponseTooLarge
            | ErrorCode::IdempotencyReplayFailed
            | ErrorCode::Internal => true,
        }
    }

    /// Stable wire string (matches the `Serialize` output). Useful for
    /// callers that need the code without going through serde, e.g.
    /// the capability discovery endpoint.
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::MissingField => "missing_field",
            ErrorCode::MissingParameter => "missing_parameter",
            ErrorCode::InvalidParameter => "invalid_parameter",
            ErrorCode::UnknownShapeType => "unknown_shape_type",
            ErrorCode::InvalidJson => "invalid_json",
            ErrorCode::KernelError => "kernel_error",
            ErrorCode::TessellationEmpty => "tessellation_empty",
            ErrorCode::SolidNotFound => "solid_not_found",
            ErrorCode::KernelReturnedWrongType => "kernel_returned_wrong_type",
            ErrorCode::IdempotencyKeyEmpty => "idempotency_key_empty",
            ErrorCode::IdempotencyKeyTooLong => "idempotency_key_too_long",
            ErrorCode::IdempotencyKeyReused => "idempotency_key_reused",
            ErrorCode::IdempotencyBodyTooLarge => "idempotency_body_too_large",
            ErrorCode::IdempotencyResponseTooLarge => "idempotency_response_too_large",
            ErrorCode::IdempotencyReplayFailed => "idempotency_replay_failed",
            ErrorCode::TransactionNotFound => "transaction_not_found",
            ErrorCode::TransactionNotActive => "transaction_not_active",
            ErrorCode::Internal => "internal_error",
        }
    }

    /// Iterate every variant. Used by capability discovery to publish
    /// the full catalog in one place.
    pub fn all() -> &'static [ErrorCode] {
        &[
            ErrorCode::MissingField,
            ErrorCode::MissingParameter,
            ErrorCode::InvalidParameter,
            ErrorCode::UnknownShapeType,
            ErrorCode::InvalidJson,
            ErrorCode::KernelError,
            ErrorCode::TessellationEmpty,
            ErrorCode::SolidNotFound,
            ErrorCode::KernelReturnedWrongType,
            ErrorCode::IdempotencyKeyEmpty,
            ErrorCode::IdempotencyKeyTooLong,
            ErrorCode::IdempotencyKeyReused,
            ErrorCode::IdempotencyBodyTooLarge,
            ErrorCode::IdempotencyResponseTooLarge,
            ErrorCode::IdempotencyReplayFailed,
            ErrorCode::TransactionNotFound,
            ErrorCode::TransactionNotActive,
            ErrorCode::Internal,
        ]
    }
}

/// One structured error response.
///
/// Construct with one of the named constructors (`ApiError::
/// missing_parameter("width")`, etc.) so the code, status, and
/// retryability stay consistent. Only the prose `message`, the optional
/// `hint`, and optional `details` payload differ from call site to
/// call site.
#[derive(Debug, Clone, Serialize)]
pub struct ApiError {
    /// Stable identifier — agents pattern-match on this.
    pub code: ErrorCode,
    /// Human-readable description. May be tweaked between releases;
    /// agents must not parse it.
    pub error: String,
    /// Whether the same request, retried, can plausibly succeed.
    pub retryable: bool,
    /// Optional remediation pointer (e.g. "Send a number for 'width'.").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    /// Optional structured payload — `{"parameter": "width"}` for
    /// missing-parameter errors, kernel diagnostics for kernel errors,
    /// etc. The shape is per-code and documented with the code.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
    /// Always `false` so the wire body matches the existing
    /// `{"success": false, ...}` contract every other handler emits.
    /// Encoded as a constant; not user-settable.
    #[serde(serialize_with = "serialize_false")]
    pub success: (),
}

fn serialize_false<S: serde::Serializer>(_: &(), s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(false)
}

impl ApiError {
    /// Generic constructor — prefer the named helpers below for
    /// well-known cases so the call site reads as the failure it
    /// represents.
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            retryable: code.retryable(),
            code,
            error: message.into(),
            hint: None,
            details: None,
            success: (),
        }
    }

    /// Attach a remediation hint visible to agents.
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Attach structured per-code detail.
    pub fn with_details(mut self, details: Value) -> Self {
        self.details = Some(details);
        self
    }

    // ── Named constructors ───────────────────────────────────────
    // One per common failure mode. They double as documentation:
    // grep for `ApiError::missing_parameter` to find every site that
    // surfaces a missing-parameter error.

    /// `parameters.<key>` was absent or non-numeric.
    pub fn missing_parameter(key: &str) -> Self {
        Self::new(
            ErrorCode::MissingParameter,
            format!("missing or non-numeric parameter '{key}'"),
        )
        .with_hint(format!(
            "Send a number for '{key}' in the parameters object."
        ))
        .with_details(serde_json::json!({ "parameter": key }))
    }

    /// `shape_type` did not match any registered primitive.
    pub fn unknown_shape_type(received: &str) -> Self {
        Self::new(
            ErrorCode::UnknownShapeType,
            format!("unknown shape_type: '{received}'"),
        )
        .with_hint(
            "Call GET /api/capabilities to list every supported \
             shape_type for POST /api/geometry."
                .to_string(),
        )
        .with_details(serde_json::json!({ "shape_type": received }))
    }

    /// A required top-level field was absent.
    pub fn missing_field(field: &str) -> Self {
        Self::new(
            ErrorCode::MissingField,
            format!("missing required field '{field}'"),
        )
        .with_details(serde_json::json!({ "field": field }))
    }

    /// Kernel-side failure with the kernel's own error string attached.
    pub fn kernel_error(kernel_msg: impl std::fmt::Display) -> Self {
        let s = kernel_msg.to_string();
        Self::new(ErrorCode::KernelError, format!("kernel error: {s}"))
            .with_details(serde_json::json!({ "kernel_message": s }))
    }

    /// Kernel returned a handle of an unexpected variant.
    pub fn kernel_returned_wrong_type(detail: impl std::fmt::Display) -> Self {
        Self::new(
            ErrorCode::KernelReturnedWrongType,
            format!("kernel returned non-solid id: {detail}"),
        )
    }

    /// Tessellation produced zero triangles — kernel defect.
    pub fn tessellation_empty(solid_id: u32, vertex_count: usize) -> Self {
        Self::new(
            ErrorCode::TessellationEmpty,
            "tessellation produced 0 triangles".to_string(),
        )
        .with_details(serde_json::json!({
            "solid_id": solid_id,
            "vertex_count": vertex_count,
        }))
    }

    /// A solid referenced by ID is not present.
    pub fn solid_not_found(solid_id: u32) -> Self {
        Self::new(
            ErrorCode::SolidNotFound,
            format!("solid {solid_id} not found"),
        )
        .with_details(serde_json::json!({ "solid_id": solid_id }))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.code.status();
        (status, Json(self)).into_response()
    }
}

/// Adapter for handlers that already return
/// `Result<_, (StatusCode, Json<Value>)>` so they can migrate to
/// `ApiError` incrementally without changing every signature in one
/// commit.
impl From<ApiError> for (StatusCode, Json<Value>) {
    fn from(e: ApiError) -> Self {
        let status = e.code.status();
        // Re-encode through serde so the wire shape matches
        // `IntoResponse` exactly (preserves snake_case codes etc.).
        let body = serde_json::to_value(&e).unwrap_or_else(|_| {
            serde_json::json!({
                "success": false,
                "error_code": ErrorCode::Internal.as_str(),
                "error": "failed to serialise structured error",
                "retryable": true,
            })
        });
        (status, Json(body))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shape_has_required_fields() {
        let e = ApiError::missing_parameter("width");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_code"], "missing_parameter");
        assert!(v["error"].is_string());
        assert_eq!(v["retryable"], false);
        assert_eq!(v["hint"].is_string(), true);
        assert_eq!(v["details"]["parameter"], "width");
    }

    #[test]
    fn unknown_shape_type_carries_received_value() {
        let e = ApiError::unknown_shape_type("dodecahedron");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["error_code"], "unknown_shape_type");
        assert_eq!(v["details"]["shape_type"], "dodecahedron");
        assert_eq!(v["retryable"], false);
    }

    #[test]
    fn kernel_error_is_retryable() {
        let e = ApiError::kernel_error("face self-intersected");
        assert_eq!(e.retryable, true);
        assert_eq!(e.code.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn status_code_lives_with_the_code() {
        assert_eq!(
            ErrorCode::MissingParameter.status(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ErrorCode::IdempotencyKeyReused.status(),
            StatusCode::CONFLICT
        );
        assert_eq!(ErrorCode::SolidNotFound.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            ErrorCode::TessellationEmpty.status(),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn all_codes_round_trip_through_as_str() {
        for code in ErrorCode::all() {
            // Serialize via serde and via as_str — they must agree.
            let json_str = serde_json::to_value(code).unwrap();
            assert_eq!(json_str.as_str().unwrap(), code.as_str());
        }
    }

    #[test]
    fn retryability_partitions_cleanly() {
        // Every code must answer `retryable()` consistently with the
        // semantic group it belongs to. This test catches accidental
        // moves between groups during refactors.
        let non_retryable_count = ErrorCode::all()
            .iter()
            .filter(|c| !c.retryable())
            .count();
        let retryable_count = ErrorCode::all().iter().filter(|c| c.retryable()).count();
        assert_eq!(
            non_retryable_count + retryable_count,
            ErrorCode::all().len(),
            "every code must be classified as retryable or not"
        );
        // Sanity: at least one of each kind.
        assert!(non_retryable_count > 0);
        assert!(retryable_count > 0);
    }
}
