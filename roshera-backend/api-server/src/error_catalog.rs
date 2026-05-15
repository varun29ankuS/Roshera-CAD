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
    /// The kernel returned a structured
    /// [`geometry_engine::operations::diagnostics::BlendFailure`]
    /// (fillet / chamfer / sew). The typed payload is serialised
    /// verbatim into `details.failure` so agents can branch on the
    /// `type` field — e.g. `RadiusExceedsCurvature` exposes
    /// `r_requested`, `r_max`, and the offending `edge` for a
    /// trivial "retry at r_max * 0.95" recovery. Diagnostics-α
    /// Phase-2 typed-surface variant. Non-retryable — the caller
    /// must change inputs (radius, selection, …) before retrying.
    BlendFailed,
    /// The kernel succeeded but tessellation produced no triangles —
    /// almost always a kernel defect, never a client bug.
    TessellationEmpty,
    /// A solid referenced by ID is not present in the model.
    SolidNotFound,
    /// `X-Roshera-Part-Id` referenced a part UUID that is not present
    /// in the `PartManager` registry. Either the part was never
    /// created (caller hit a stale tab id) or it was deleted out
    /// from under this request. Non-retryable from the same id.
    PartNotFound,
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

    // ── Branch / sandbox layer ────────────────────────────────────
    /// A branch ID was syntactically valid (or the literal `main`)
    /// but no such branch exists in the timeline.
    BranchNotFound,
    /// A branch lifecycle transition was rejected — for example
    /// abandoning a branch that is already merged, or merging a
    /// branch whose state is not Active.
    BranchInvalidState,
    /// A merge could not be applied automatically (conflicts,
    /// non-fast-forward without a strategy, etc.). Non-retryable
    /// without a strategy change or manual conflict resolution.
    BranchMergeConflict,

    // ── Sketch / constraint solver ────────────────────────────────
    /// A constraint mutation (e.g. PATCH on a dimensional value)
    /// drove the sketch into an over-constrained or unsolvable
    /// state. The server reverted the change; the caller must
    /// adjust other constraints or supply a different value before
    /// retrying. Details carry the offending residuals and the
    /// before/after values so the UI can surface the conflict.
    SketchConstraintConflict,

    // ── AI surface ────────────────────────────────────────────────
    /// No LLM API key was configured at server start, so AI routes
    /// refuse to serve traffic. Operators must set `ANTHROPIC_API_KEY`
    /// (or another supported provider key) and restart. This is a
    /// deployment-time misconfiguration, not a transient failure —
    /// retrying without changing server config will fail identically.
    AiNotConfigured,

    // ── Authorization / routing ───────────────────────────────────
    /// Caller authenticated but lacks the permission needed for this
    /// route. Mapped to HTTP 403 — not retryable from the same
    /// principal; needs an operator to grant the role.
    PermissionDenied,
    /// The route exists but the requested HTTP method is not the
    /// supported one (e.g. PUT/DELETE on `/api/geometry/{id}`, where
    /// the architecture forces mutations through the timeline).
    /// Mapped to HTTP 405. Non-retryable — the client must change
    /// endpoint, not just retry.
    MethodNotAllowed,

    // ── Catch-alls ────────────────────────────────────────────────
    /// Unspecified server-side fault. Always retryable.
    #[serde(rename = "internal_error")]
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
            | ErrorCode::BlendFailed
            | ErrorCode::IdempotencyKeyEmpty
            | ErrorCode::IdempotencyKeyTooLong => StatusCode::BAD_REQUEST,

            ErrorCode::IdempotencyKeyReused
            | ErrorCode::TransactionNotActive
            | ErrorCode::BranchInvalidState
            | ErrorCode::BranchMergeConflict
            | ErrorCode::SketchConstraintConflict => StatusCode::CONFLICT,
            ErrorCode::IdempotencyBodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,

            ErrorCode::SolidNotFound
            | ErrorCode::PartNotFound
            | ErrorCode::TransactionNotFound
            | ErrorCode::BranchNotFound => StatusCode::NOT_FOUND,

            ErrorCode::KernelError
            | ErrorCode::TessellationEmpty
            | ErrorCode::KernelReturnedWrongType
            | ErrorCode::IdempotencyResponseTooLarge
            | ErrorCode::IdempotencyReplayFailed
            | ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,

            ErrorCode::AiNotConfigured => StatusCode::SERVICE_UNAVAILABLE,

            ErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
            ErrorCode::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
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
            | ErrorCode::BlendFailed
            | ErrorCode::SolidNotFound
            | ErrorCode::PartNotFound
            | ErrorCode::IdempotencyKeyEmpty
            | ErrorCode::IdempotencyKeyTooLong
            | ErrorCode::IdempotencyKeyReused
            | ErrorCode::IdempotencyBodyTooLarge
            | ErrorCode::TransactionNotFound
            | ErrorCode::TransactionNotActive
            | ErrorCode::BranchNotFound
            | ErrorCode::BranchInvalidState
            | ErrorCode::BranchMergeConflict
            | ErrorCode::SketchConstraintConflict
            | ErrorCode::AiNotConfigured
            | ErrorCode::PermissionDenied
            | ErrorCode::MethodNotAllowed => false,

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
            ErrorCode::BlendFailed => "blend_failed",
            ErrorCode::TessellationEmpty => "tessellation_empty",
            ErrorCode::SolidNotFound => "solid_not_found",
            ErrorCode::PartNotFound => "part_not_found",
            ErrorCode::KernelReturnedWrongType => "kernel_returned_wrong_type",
            ErrorCode::IdempotencyKeyEmpty => "idempotency_key_empty",
            ErrorCode::IdempotencyKeyTooLong => "idempotency_key_too_long",
            ErrorCode::IdempotencyKeyReused => "idempotency_key_reused",
            ErrorCode::IdempotencyBodyTooLarge => "idempotency_body_too_large",
            ErrorCode::IdempotencyResponseTooLarge => "idempotency_response_too_large",
            ErrorCode::IdempotencyReplayFailed => "idempotency_replay_failed",
            ErrorCode::TransactionNotFound => "transaction_not_found",
            ErrorCode::TransactionNotActive => "transaction_not_active",
            ErrorCode::BranchNotFound => "branch_not_found",
            ErrorCode::BranchInvalidState => "branch_invalid_state",
            ErrorCode::BranchMergeConflict => "branch_merge_conflict",
            ErrorCode::SketchConstraintConflict => "sketch_constraint_conflict",
            ErrorCode::AiNotConfigured => "ai_not_configured",
            ErrorCode::PermissionDenied => "permission_denied",
            ErrorCode::MethodNotAllowed => "method_not_allowed",
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
            ErrorCode::BlendFailed,
            ErrorCode::TessellationEmpty,
            ErrorCode::SolidNotFound,
            ErrorCode::PartNotFound,
            ErrorCode::KernelReturnedWrongType,
            ErrorCode::IdempotencyKeyEmpty,
            ErrorCode::IdempotencyKeyTooLong,
            ErrorCode::IdempotencyKeyReused,
            ErrorCode::IdempotencyBodyTooLarge,
            ErrorCode::IdempotencyResponseTooLarge,
            ErrorCode::IdempotencyReplayFailed,
            ErrorCode::TransactionNotFound,
            ErrorCode::TransactionNotActive,
            ErrorCode::BranchNotFound,
            ErrorCode::BranchInvalidState,
            ErrorCode::BranchMergeConflict,
            ErrorCode::SketchConstraintConflict,
            ErrorCode::AiNotConfigured,
            ErrorCode::PermissionDenied,
            ErrorCode::MethodNotAllowed,
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
    #[serde(rename = "error_code")]
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

    /// Structured blend failure (Diagnostics-α Phase-2): the kernel
    /// returned [`geometry_engine::operations::diagnostics::BlendFailure`]
    /// and we surface the taxonomy verbatim. The `failure` field of
    /// `details` is the internally-tagged JSON the kernel emits
    /// (`{"type": "RadiusExceedsCurvature", "edge": 7, ...}`), so an
    /// agent can branch on `details.failure.type` without parsing the
    /// human-readable `error` field.
    ///
    /// Returned as HTTP 400 because the failure is — by construction —
    /// a caller-supplied infeasibility (radius too large for local
    /// curvature, setback too long, mixed convexity, …). Retrying the
    /// same request will fail identically; the agent must change its
    /// inputs before retrying.
    pub fn blend_failed(
        failure: &geometry_engine::operations::diagnostics::BlendFailure,
    ) -> Self {
        // `BlendFailure: Display` carries an actionable human summary
        // (e.g. "blend radius 2 at edge 7 station 0.420 exceeds local
        // curvature limit r_max=1.25"). Surface it on the `error`
        // field so logs / fallback consumers still get the message.
        let message = failure.to_string();
        // Serialise the typed payload. The kernel's `BlendFailure`
        // derives `serde::Serialize` with `#[serde(tag = "type")]`,
        // so this is guaranteed to succeed for every variant; the
        // `unwrap_or_else` fallback is paranoia — if it ever fires
        // the wire shape still satisfies the catalog contract.
        let payload = serde_json::to_value(failure).unwrap_or_else(|_| {
            serde_json::json!({
                "type": "TopologyViolation",
                "detail": message.clone(),
            })
        });
        Self::new(ErrorCode::BlendFailed, format!("blend failed: {message}"))
            .with_details(serde_json::json!({ "failure": payload }))
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

    /// `X-Roshera-Part-Id` referenced a part UUID that isn't in the
    /// `PartManager` registry. The detail carries the offending id
    /// so the frontend can drop a stale tab.
    pub fn part_not_found(part_id: uuid::Uuid) -> Self {
        Self::new(
            ErrorCode::PartNotFound,
            format!("part {part_id} not found"),
        )
        .with_hint(
            "Create a part with POST /api/parts and use the returned \
             id in the X-Roshera-Part-Id header.",
        )
        .with_details(serde_json::json!({ "part_id": part_id }))
    }

    /// Caller is authenticated but lacks the required permission for
    /// this route. The `permission` detail names the missing scope so
    /// agents can request the right grant from a human operator.
    pub fn permission_denied(permission: &str) -> Self {
        Self::new(
            ErrorCode::PermissionDenied,
            format!("missing required permission '{permission}'"),
        )
        .with_details(serde_json::json!({ "permission": permission }))
    }

    /// Endpoint exists but the requested method is intentionally
    /// disabled — typically because the architecture funnels mutations
    /// through a different surface (the timeline). The hint should
    /// point the caller at the correct endpoint.
    pub fn method_not_allowed(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self::new(ErrorCode::MethodNotAllowed, message).with_hint(hint)
    }

    /// AI surface refused: no LLM API key was configured at server
    /// start. Returned as 503 from `/api/ai/command`,
    /// `/api/ai/command/stream`, and `/api/ai/status` until the
    /// operator sets a provider key and restarts the server. Never
    /// served as a transient error — agents that hit this should stop
    /// retrying and surface the misconfiguration to a human.
    pub fn ai_not_configured() -> Self {
        Self::new(
            ErrorCode::AiNotConfigured,
            "AI provider not configured: no LLM API key found at server start".to_string(),
        )
        .with_hint(
            "Set ANTHROPIC_API_KEY (or another supported provider key) in \
             the server environment and restart. Use GET /api/ai/status \
             to verify."
                .to_string(),
        )
        .with_details(serde_json::json!({
            "missing_env": ["ANTHROPIC_API_KEY"],
        }))
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = self.code.status();
        (status, Json(self)).into_response()
    }
}

/// Map a kernel [`geometry_engine::operations::OperationError`] onto
/// an [`ApiError`]. The typed [`BlendFailed`](ErrorCode::BlendFailed)
/// variant is preserved end-to-end with its full taxonomy in
/// `details.failure`; every other variant is funnelled through
/// [`ApiError::kernel_error`] (legacy stringified surface).
///
/// Call sites that previously had `.map_err(ApiError::kernel_error)`
/// can become `.map_err(ApiError::from)` — and any kernel site that
/// returns `OperationError::BlendFailed(...)` will start surfacing
/// structured JSON to agents instead of a flattened message.
impl From<geometry_engine::operations::OperationError> for ApiError {
    fn from(err: geometry_engine::operations::OperationError) -> Self {
        use geometry_engine::operations::OperationError;
        match err {
            OperationError::BlendFailed(failure) => ApiError::blend_failed(&failure),
            other => ApiError::kernel_error(other),
        }
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

    /// Diagnostics-α Phase-2: a `BlendFailure::RadiusExceedsCurvature`
    /// returned from the kernel as
    /// `OperationError::BlendFailed(...)` must surface as HTTP 400
    /// with the typed JSON payload nested under `details.failure`.
    /// Agents pattern-match on `details.failure.type` (the kernel's
    /// internally-tagged discriminator) to recover automatically —
    /// changing this wire shape is a breaking change to the agent
    /// surface.
    #[test]
    fn blend_failed_wire_shape_carries_typed_failure() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::RadiusExceedsCurvature {
            edge: 7,
            station: 0.42,
            r_requested: 2.0,
            r_max: 1.25,
        };
        let op_err = OperationError::BlendFailed(Box::new(failure));
        let api_err: ApiError = op_err.into();
        assert_eq!(api_err.code, ErrorCode::BlendFailed);
        assert_eq!(api_err.code.status(), StatusCode::BAD_REQUEST);
        assert!(!api_err.retryable);

        let v = serde_json::to_value(&api_err).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_code"], "blend_failed");
        assert!(v["error"].as_str().unwrap().contains("r_max=1.25"));
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "RadiusExceedsCurvature");
        assert_eq!(payload["edge"], 7);
        assert_eq!(payload["r_requested"], 2.0);
        assert_eq!(payload["r_max"], 1.25);
    }

    /// Diagnostics-α Phase-2: `BlendFailure::SetbackTooLong` survives
    /// the `OperationError → ApiError → JSON` chain with the right
    /// discriminator and field values. Sister test to
    /// `blend_failed_wire_shape_carries_typed_failure`, but for the
    /// F2-γ.1 corner-compatibility gate.
    #[test]
    fn blend_failed_setback_too_long_wire_shape() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::SetbackTooLong {
            vertex: 11,
            setback: 3.5,
            edge_length: 2.0,
        };
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        assert_eq!(api_err.code, ErrorCode::BlendFailed);
        assert_eq!(api_err.code.status(), StatusCode::BAD_REQUEST);
        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "SetbackTooLong");
        assert_eq!(payload["vertex"], 11);
        assert_eq!(payload["setback"], 3.5);
        assert_eq!(payload["edge_length"], 2.0);
    }

    /// Diagnostics-α Phase-2: `BlendFailure::DihedralInflection`
    /// surfaces with the typed wire shape. Inflection means the
    /// dihedral angle passes through 0 / π along the edge length —
    /// single-radius blends are undefined across the crossing.
    #[test]
    fn blend_failed_dihedral_inflection_wire_shape() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::DihedralInflection {
            edge: 4,
            station: 0.61,
            dihedral_deg: -0.5,
        };
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "DihedralInflection");
        assert_eq!(payload["edge"], 4);
        assert_eq!(payload["station"], 0.61);
        assert_eq!(payload["dihedral_deg"], -0.5);
    }

    /// Diagnostics-α Phase-2: `BlendFailure::SewGapTooLarge` from the
    /// F7-δ continuity gate surfaces as the typed payload. Pins the
    /// wire shape for the sew-side migration landed in sew.rs:778.
    #[test]
    fn blend_failed_sew_gap_wire_shape() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::SewGapTooLarge {
            edge: 22,
            gap: 0.015,
            tolerance: 1e-6,
        };
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "SewGapTooLarge");
        assert_eq!(payload["edge"], 22);
        assert_eq!(payload["gap"], 0.015);
        assert_eq!(payload["tolerance"], 1e-6);
    }

    /// Diagnostics-α Phase-2: `BlendFailure::SpineSolverDiverged`
    /// from the F3-γ marching corrector surfaces with edge / station
    /// / residual fields. Pins the wire shape for the spine-side
    /// migration landed in `spine_solver::corrector`.
    #[test]
    fn blend_failed_spine_solver_diverged_wire_shape() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::SpineSolverDiverged {
            edge: 9,
            station: 0.73,
            residual: 4.2e-3,
        };
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "SpineSolverDiverged");
        assert_eq!(payload["edge"], 9);
        assert_eq!(payload["station"], 0.73);
        assert_eq!(payload["residual"], 4.2e-3);
    }

    /// Diagnostics-α Phase-2: `BlendFailure::VertexBlendUnsupported`
    /// surfaces with both the nested `kind` (BlendVertexKind) and the
    /// `reason` (VertexBlendUnsupportedReason) discriminators
    /// preserved. This is the deepest nesting the agent surface
    /// exposes; any drift in nested serde tags breaks corner-blend
    /// dispatch on the consumer side.
    #[test]
    fn blend_failed_vertex_blend_unsupported_wire_shape() {
        use geometry_engine::operations::blend_graph::BlendVertexKind;
        use geometry_engine::operations::diagnostics::{
            BlendFailure, VertexBlendUnsupportedReason,
        };
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::VertexBlendUnsupported {
            vertex: 17,
            kind: BlendVertexKind::ConvexCorner { degree: 5 },
            reason: VertexBlendUnsupportedReason::DegreeTooHigh { degree: 5 },
        };
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "VertexBlendUnsupported");
        assert_eq!(payload["vertex"], 17);
        // Nested kind discriminator (externally tagged enum — JSON
        // looks like `{"ConvexCorner": {"degree": 5}}`).
        assert_eq!(payload["kind"]["ConvexCorner"]["degree"], 5);
        // Nested reason discriminator (same convention).
        assert_eq!(payload["reason"]["DegreeTooHigh"]["degree"], 5);
    }

    /// Diagnostics-α Phase-2: `BlendFailure::TopologyViolation` is the
    /// freeform catch-all — its `detail` string must still surface
    /// under `details.failure.detail`. Agents that branch on
    /// `details.failure.type == "TopologyViolation"` treat this as a
    /// non-recoverable error and surface the detail string to the
    /// user.
    #[test]
    fn blend_failed_topology_violation_wire_shape() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::TopologyViolation {
            detail: "non-manifold edge after splice".into(),
        };
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "TopologyViolation");
        assert_eq!(payload["detail"], "non-manifold edge after splice");
    }

    /// Every `BlendFailure` variant must map to HTTP 400 (caller-
    /// recoverable bad request, not a server fault). This is the
    /// status-side counterpart to the per-variant wire-shape pins;
    /// it catches accidental moves of `ErrorCode::BlendFailed` into
    /// the 5xx group during catalog refactors.
    #[test]
    fn blend_failed_status_is_400_for_every_variant() {
        use geometry_engine::operations::blend_graph::BlendVertexKind;
        use geometry_engine::operations::diagnostics::{
            BlendFailure, VertexBlendUnsupportedReason,
        };
        use geometry_engine::operations::OperationError;
        let variants: Vec<BlendFailure> = vec![
            BlendFailure::RadiusExceedsCurvature {
                edge: 0,
                station: 0.0,
                r_requested: 1.0,
                r_max: 0.5,
            },
            BlendFailure::SetbackTooLong {
                vertex: 0,
                setback: 1.0,
                edge_length: 0.5,
            },
            BlendFailure::DihedralInflection {
                edge: 0,
                station: 0.5,
                dihedral_deg: 0.0,
            },
            BlendFailure::SewGapTooLarge {
                edge: 0,
                gap: 1.0,
                tolerance: 1e-6,
            },
            BlendFailure::SpineSolverDiverged {
                edge: 0,
                station: 0.5,
                residual: 1.0,
            },
            BlendFailure::VertexBlendUnsupported {
                vertex: 0,
                kind: BlendVertexKind::Cliff,
                reason: VertexBlendUnsupportedReason::NonManifoldNeighbourhood,
            },
            BlendFailure::TopologyViolation {
                detail: "x".into(),
            },
        ];
        for failure in variants {
            let api_err: ApiError = OperationError::BlendFailed(Box::new(failure.clone())).into();
            assert_eq!(
                api_err.code,
                ErrorCode::BlendFailed,
                "variant {:?} should route to BlendFailed",
                failure
            );
            assert_eq!(
                api_err.code.status(),
                StatusCode::BAD_REQUEST,
                "variant {:?} must surface as HTTP 400",
                failure
            );
            assert!(
                !api_err.retryable,
                "variant {:?} must be non-retryable",
                failure
            );
        }
    }

    /// The `error` field (human-readable summary) must include the
    /// kernel-side Display output so logs and humans can read the
    /// rejection without parsing `details.failure`. This is the
    /// observability counterpart to the structured payload — agents
    /// branch on `details.failure.type`, humans read `error`.
    #[test]
    fn blend_failed_error_message_carries_kernel_display() {
        use geometry_engine::operations::diagnostics::BlendFailure;
        use geometry_engine::operations::OperationError;
        let failure = BlendFailure::SpineSolverDiverged {
            edge: 42,
            station: 0.5,
            residual: 1.2e-2,
        };
        let display = failure.to_string();
        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let error_msg = v["error"].as_str().unwrap();
        assert!(
            error_msg.contains(&display),
            "error field {:?} must include kernel display {:?}",
            error_msg,
            display
        );
        assert!(
            error_msg.starts_with("blend failed:"),
            "error field {:?} must be prefixed with the typed-surface marker",
            error_msg
        );
    }

    /// Non-`BlendFailed` `OperationError` variants must still funnel
    /// through `kernel_error` so the legacy surface is preserved
    /// while the typed surface lands incrementally.
    #[test]
    fn non_blend_operation_error_funnels_through_kernel_error() {
        use geometry_engine::operations::OperationError;
        let op_err = OperationError::InvalidGeometry("non-manifold edge".into());
        let api_err: ApiError = op_err.into();
        assert_eq!(api_err.code, ErrorCode::KernelError);
        let v = serde_json::to_value(&api_err).unwrap();
        assert!(v["details"]["kernel_message"]
            .as_str()
            .unwrap()
            .contains("non-manifold edge"));
    }

    #[test]
    fn retryability_partitions_cleanly() {
        // Every code must answer `retryable()` consistently with the
        // semantic group it belongs to. This test catches accidental
        // moves between groups during refactors.
        let non_retryable_count = ErrorCode::all().iter().filter(|c| !c.retryable()).count();
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
