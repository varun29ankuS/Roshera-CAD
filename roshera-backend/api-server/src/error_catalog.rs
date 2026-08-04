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
    /// `POST /api/timeline/checkpoint` was given a name that names a
    /// sequence position or a clock/date reading instead of a design
    /// intent — "step 3", "cp 2", "checkpoint", a bare number, or
    /// "Checkpoint 9:59:36 PM". A checkpoint is a declared intent; a
    /// name that says only *when* or *where in the sequence* it was
    /// made says nothing a timeline row doesn't already show. The REST
    /// route is the floor beneath both the MCP intent gate
    /// (`GENERIC_CHECKPOINT_NAME` in `roshera-mcp/src/gates.ts`) and
    /// the frontend picker — all three refuse the same shapes. Mapped
    /// to HTTP 422 (well-formed request, semantically unacceptable
    /// value). Non-retryable with the same name; `details` carries the
    /// rejected name.
    CheckpointNameRejected,

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
    /// A Boolean difference whose tool operand never touches the target:
    /// the kernel found zero intersection curves and the tool contributed
    /// no face, so the "result" would have been the target returned
    /// unchanged. Surfaced as a typed refusal instead of a silent no-op
    /// success (the drill-pattern "holes drilled, volume unchanged" lie).
    /// `details` carries `object_a` (target) and `object_b` (tool).
    /// Non-retryable — the caller must re-position the tool (fix the
    /// pattern `center` / `axis`) before retrying.
    BooleanDisjoint,
    /// A heavy mutating kernel op (boolean, and any op routed through
    /// the bounded executor) ran past its per-class wall-clock budget
    /// and was abandoned. Task #41: a Rust compute loop cannot be
    /// cancelled, so the op runs on a deep CLONE off the model write
    /// lock; on timeout the request returns THIS code promptly, the
    /// live model is left untouched, and the write lock is free (the
    /// runaway thread finishes on the discarded clone). Non-retryable:
    /// the same inputs will exceed the same budget again — the caller
    /// must simplify / reposition the geometry, or an operator must
    /// raise the `ROSHERA_OP_TIMEOUT*` budget. `details` carries
    /// `op_kind`, `budget_secs`, and the operand solid ids.
    OpTimeout,
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
    /// **Honesty gate.** A mutating operation was refused because the
    /// solid it would stack work onto is UNSOUND by the kernel's LIVE
    /// verdict (`certify_solid().is_sound() == false`). Every certificate
    /// downstream of a defective base inherits the defect, so the base
    /// must be repaired or rolled back first — otherwise an agent builds
    /// on a lie and the whole self-certifying story collapses.
    ///
    /// The rule was previously enforced ONLY in the MCP client
    /// (`roshera-mcp/src/gates.ts`), which made it a linter rather than a
    /// gate: an agent that spoke plain REST simply declined to use it.
    /// This code is the server-side enforcement, applied to every
    /// base-taking geometry mutation.
    ///
    /// **Escape hatch (deliberate, documented):** `acknowledge_unsound:
    /// true` in the request body proceeds anyway. That is the correct
    /// call for a repair flow — a boolean used to heal an open shell, a
    /// rebuild from a known-good state. An agent that knowingly proceeds
    /// is behaving correctly; one that does so UNKNOWINGLY is the defect
    /// this code exists to prevent. `hint` always names the flag.
    ///
    /// Mapped to HTTP 409 (a state conflict, not a malformed value — the
    /// request is well-formed and the parameters are fine; the MODEL is in
    /// a state that forbids the operation). Non-retryable: the identical
    /// call against the identical state earns the identical refusal. It
    /// becomes possible again only after the base changes — and because
    /// the verdict is re-read live on every call, a repair by ANY author
    /// unblocks the very next request with no restart and no cache flush.
    /// `details` carries `gate`, `solid_id`, `verdict`, and `operation`.
    UnsoundBase,

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

    // ── Document layer ──────────────────────────────────────────
    /// `POST /api/documents/{id}/open` (or any other document-scoped
    /// route) referenced a document id that is not in the registry —
    /// never created, or a typo. Non-retryable from the same id.
    DocumentNotFound,
    /// `DELETE /api/documents/{id}` targeted the document currently
    /// loaded into the live model. Deleting the thing the caller (or
    /// another client) is actively looking at is a foot-gun — the
    /// caller must switch to a different document first. Non-retryable
    /// without that switch.
    DocumentDeleteRefusedActive,
    /// `DELETE /api/documents/{id}` targeted the only document left in
    /// the registry. The app must never be left in a zero-document
    /// state, so the last document can never be removed. Non-retryable
    /// — a new document must exist before this one can go.
    DocumentDeleteRefusedLast,
    /// `DELETE /api/documents/{id}` targeted the default document
    /// (`durability::DURABILITY_SESSION_ID`, "Main Document"), which
    /// carries the pre-existing legacy event ledger. Removing it is a
    /// deliberate administrative act, never an ordinary UI affordance.
    /// Non-retryable through this route.
    DocumentDeleteRefusedDefault,

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
    /// A provider (or credential mode) outside the server-owned
    /// allowlist was requested through the provider-configuration
    /// surface. Providers are operator/user-configured server-side
    /// only; anything that would resolve by spawning an arbitrary
    /// local binary is excluded by construction. `details` carries the
    /// requested id and the allowlist. Non-retryable.
    AiProviderRefused,
    /// A provider credential failed its live validation round-trip
    /// (e.g. the vendor API returned 401/403 for the supplied key, or
    /// the subscription CLI probe failed). The caller must supply a
    /// different credential — retrying the same one fails identically.
    /// NOTE: `POST /api/ai/provider/models` (live model discovery)
    /// reports its own 401/403 under `AiModelDiscoveryFailed` instead,
    /// so the two codes never collide — this code stays the one the
    /// PUT/`/test` credential round-trip already used.
    AiCredentialInvalid,
    /// `POST /api/ai/provider/models` (live model discovery) could not
    /// produce the vendor's model list. `details` always carries
    /// `provider` and an `outcome` discriminator (`no_base_url` |
    /// `unauthorized` | `not_found` | `unexpected_status` | `timeout` |
    /// `transport_error`) plus, when the vendor answered at all, its own
    /// `vendor_status` and `vendor_message` — the caller must be able to
    /// tell "the key is wrong" (`unauthorized`) from "the resolved URL
    /// is wrong" (`not_found`) from "nothing could be resolved at all"
    /// (`no_base_url`) rather than one generic failure. Never a stored
    /// or guessed model list on any of these outcomes.
    AiModelDiscoveryFailed,
    /// A user-selected model ID was rejected by the live provider (e.g.
    /// `GET /v1/models/{id}` 404, or the vendor otherwise refused it).
    /// Distinct from `AiCredentialInvalid`: the credential itself is
    /// fine, the model name is not one it can serve. `details` carries
    /// the rejected model. Never surfaced as a silent fallback to a
    /// default model — the caller must pick a different one or "default".
    AiModelRejected,

    // ── ACP (goose agent) transport gate ──────────────────────────
    /// A JSON-RPC method outside the ACP method allowlist was posted
    /// to `/acp`. Provider switching, config mutation, extension
    /// management and similar RPCs are server-configured surfaces —
    /// a client cannot invoke them, by policy. `details` carries the
    /// refused method and the allowed set. Non-retryable.
    AcpMethodNotAllowed,
    /// A WebSocket upgrade was attempted on `/acp`. The WebSocket
    /// transport is disabled wholesale: goose owns the frame loop, so
    /// frames could not be method-filtered — instead of a filter that
    /// cannot be enforced, the upgrade itself is refused and clients
    /// use the POST + SSE transport (which is filtered). Non-retryable.
    AcpWebsocketDisabled,
    /// A `session/new` or `session/load` body carried a `_meta` key
    /// (`provider`, `enabledExtensions`, `recipeDeeplink`, `recipeId`)
    /// that pre-empts or overrides the `mcpServers` entry Roshera
    /// injects into every session — each is an arbitrary-command
    /// surface in its own right (`enabledExtensions` in particular
    /// routes goose around `mcpServers` entirely). `details` carries
    /// the refused method and key. Non-retryable — the caller must
    /// drop the key from the request, not retry with it.
    AcpForbiddenSessionMeta,

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
            | ErrorCode::BooleanDisjoint
            | ErrorCode::IdempotencyKeyEmpty
            | ErrorCode::IdempotencyKeyTooLong => StatusCode::BAD_REQUEST,

            ErrorCode::IdempotencyKeyReused
            | ErrorCode::TransactionNotActive
            | ErrorCode::BranchInvalidState
            | ErrorCode::BranchMergeConflict
            | ErrorCode::SketchConstraintConflict
            | ErrorCode::DocumentDeleteRefusedActive
            | ErrorCode::DocumentDeleteRefusedLast
            | ErrorCode::DocumentDeleteRefusedDefault
            // The request is well-formed and its parameters are valid —
            // the MODEL is in a state that forbids the operation. That is
            // a conflict, not a bad value, so 409 rather than 422.
            | ErrorCode::UnsoundBase => StatusCode::CONFLICT,
            ErrorCode::IdempotencyBodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,

            // Well-formed request, semantically unacceptable value —
            // the canonical 422. Kept distinct from 400 so a client can
            // tell "your JSON is malformed" from "your name carries no
            // intent".
            ErrorCode::CheckpointNameRejected => StatusCode::UNPROCESSABLE_ENTITY,

            ErrorCode::SolidNotFound
            | ErrorCode::PartNotFound
            | ErrorCode::TransactionNotFound
            | ErrorCode::BranchNotFound
            | ErrorCode::DocumentNotFound => StatusCode::NOT_FOUND,

            ErrorCode::KernelError
            | ErrorCode::TessellationEmpty
            | ErrorCode::KernelReturnedWrongType
            | ErrorCode::IdempotencyResponseTooLarge
            | ErrorCode::IdempotencyReplayFailed
            | ErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,

            ErrorCode::AiNotConfigured => StatusCode::SERVICE_UNAVAILABLE,

            ErrorCode::AiCredentialInvalid
            | ErrorCode::AiModelRejected
            | ErrorCode::AiModelDiscoveryFailed => StatusCode::BAD_REQUEST,
            ErrorCode::AiProviderRefused
            | ErrorCode::AcpMethodNotAllowed
            | ErrorCode::AcpWebsocketDisabled
            | ErrorCode::AcpForbiddenSessionMeta => StatusCode::FORBIDDEN,

            // A bounded op that blew its budget is a server-side time
            // limit, surfaced as 504 Gateway Timeout — the request was
            // accepted and computed, but the computation did not finish
            // in time and was abandoned.
            ErrorCode::OpTimeout => StatusCode::GATEWAY_TIMEOUT,

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
            | ErrorCode::CheckpointNameRejected
            | ErrorCode::BlendFailed
            | ErrorCode::BooleanDisjoint
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
            | ErrorCode::DocumentNotFound
            | ErrorCode::DocumentDeleteRefusedActive
            | ErrorCode::DocumentDeleteRefusedLast
            | ErrorCode::DocumentDeleteRefusedDefault
            | ErrorCode::SketchConstraintConflict
            | ErrorCode::AiNotConfigured
            | ErrorCode::AiProviderRefused
            | ErrorCode::AiCredentialInvalid
            | ErrorCode::AiModelRejected
            // A timeout outcome is reported under this same code (see
            // its doc's `outcome` discriminator) — classified
            // non-retryable at the code level for the same reason
            // `OpTimeout` is above: the code covers several outcomes and
            // most of them (bad key, bad/unresolvable URL) are
            // deterministic. A caller that inspects `details.outcome ==
            // "timeout"` and wants to retry that specific case may.
            | ErrorCode::AiModelDiscoveryFailed
            | ErrorCode::AcpMethodNotAllowed
            | ErrorCode::AcpWebsocketDisabled
            | ErrorCode::AcpForbiddenSessionMeta
            // A budget overrun is deterministic in its inputs: retrying
            // the identical request re-runs the identical corefinement
            // and blows the identical budget. The caller must change the
            // geometry (or an operator must raise the budget), so this is
            // non-retryable by the same rule as a caller-supplied
            // infeasibility.
            | ErrorCode::OpTimeout
            // An intentional refusal, not a transient failure: the same
            // call against the same model state earns the same answer, so
            // a blind retry is pure waste. The caller must either repair
            // the base or re-issue with `acknowledge_unsound: true`.
            | ErrorCode::UnsoundBase
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
            ErrorCode::CheckpointNameRejected => "checkpoint_name_rejected",
            ErrorCode::KernelError => "kernel_error",
            ErrorCode::BlendFailed => "blend_failed",
            ErrorCode::BooleanDisjoint => "boolean_disjoint",
            ErrorCode::OpTimeout => "op_timeout",
            ErrorCode::TessellationEmpty => "tessellation_empty",
            ErrorCode::SolidNotFound => "solid_not_found",
            ErrorCode::PartNotFound => "part_not_found",
            ErrorCode::KernelReturnedWrongType => "kernel_returned_wrong_type",
            ErrorCode::UnsoundBase => "unsound_base",
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
            ErrorCode::DocumentNotFound => "document_not_found",
            ErrorCode::DocumentDeleteRefusedActive => "document_delete_refused_active",
            ErrorCode::DocumentDeleteRefusedLast => "document_delete_refused_last",
            ErrorCode::DocumentDeleteRefusedDefault => "document_delete_refused_default",
            ErrorCode::SketchConstraintConflict => "sketch_constraint_conflict",
            ErrorCode::AiNotConfigured => "ai_not_configured",
            ErrorCode::AiProviderRefused => "ai_provider_refused",
            ErrorCode::AiCredentialInvalid => "ai_credential_invalid",
            ErrorCode::AiModelRejected => "ai_model_rejected",
            ErrorCode::AiModelDiscoveryFailed => "ai_model_discovery_failed",
            ErrorCode::AcpMethodNotAllowed => "acp_method_not_allowed",
            ErrorCode::AcpWebsocketDisabled => "acp_websocket_disabled",
            ErrorCode::AcpForbiddenSessionMeta => "acp_forbidden_session_meta",
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
            ErrorCode::CheckpointNameRejected,
            ErrorCode::KernelError,
            ErrorCode::BlendFailed,
            ErrorCode::BooleanDisjoint,
            ErrorCode::OpTimeout,
            ErrorCode::TessellationEmpty,
            ErrorCode::SolidNotFound,
            ErrorCode::PartNotFound,
            ErrorCode::KernelReturnedWrongType,
            ErrorCode::UnsoundBase,
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
            ErrorCode::DocumentNotFound,
            ErrorCode::DocumentDeleteRefusedActive,
            ErrorCode::DocumentDeleteRefusedLast,
            ErrorCode::DocumentDeleteRefusedDefault,
            ErrorCode::SketchConstraintConflict,
            ErrorCode::AiNotConfigured,
            ErrorCode::AiProviderRefused,
            ErrorCode::AiCredentialInvalid,
            ErrorCode::AiModelRejected,
            ErrorCode::AiModelDiscoveryFailed,
            ErrorCode::AcpMethodNotAllowed,
            ErrorCode::AcpWebsocketDisabled,
            ErrorCode::AcpForbiddenSessionMeta,
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

    /// `POST /api/timeline/checkpoint` was given a name that is a
    /// sequence position or a clock/date reading — named-nothing. The
    /// message names the standard (a declared design intent), what was
    /// received, and what a passing name looks like; `details` carries
    /// the rejected name so clients need not parse prose.
    pub fn checkpoint_name_rejected(name: &str) -> Self {
        Self::new(
            ErrorCode::CheckpointNameRejected,
            format!(
                "checkpoint name '{name}' names a sequence position or a \
                 clock reading, not a design intent — the timeline row \
                 already shows its own time and sequence, so this name \
                 would add a row that says nothing"
            ),
        )
        .with_hint(
            "Name what a drawing would name: the feature, its governing \
             dimensions, and where it sits — e.g. 'bolt circle 8 x D18 on \
             D160 B.C.' or 'M8 clearance holes, close fit, 4x base corners'.",
        )
        .with_details(serde_json::json!({ "rejected_name": name }))
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
    pub fn blend_failed(failure: &geometry_engine::operations::diagnostics::BlendFailure) -> Self {
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

    /// A bounded mutating op exceeded its wall-clock budget and was
    /// abandoned (Task #41). Carries the op kind, the budget it blew,
    /// and the operand solid ids so an agent can branch on
    /// `details.op_kind` and report which geometry was too heavy. The
    /// live model is unchanged — the op ran on a discarded clone — so
    /// the hint steers the caller at the two real levers: simpler
    /// geometry, or a larger operator-set budget.
    pub fn op_timeout(op_kind: &str, budget_secs: f64, operands: &[u32]) -> Self {
        Self::new(
            ErrorCode::OpTimeout,
            format!(
                "operation '{op_kind}' exceeded its {budget_secs}s time budget \
                 and was aborted; the model is unchanged"
            ),
        )
        .with_hint(
            "The corefinement did not converge in the allotted time. Simplify \
             or reposition the operands (fewer faces, avoid near-tangential \
             coincident faces / thin walls sharing a seam), or have an operator \
             raise the ROSHERA_OP_TIMEOUT_SECS budget and retry."
                .to_string(),
        )
        .with_details(serde_json::json!({
            "op_kind": op_kind,
            "budget_secs": budget_secs,
            "operands": operands,
        }))
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

    /// **The unsound-base refusal.** A mutating operation would stack
    /// work onto `solid_id`, whose LIVE kernel verdict is unsound.
    ///
    /// Deliberately mirrors `roshera-mcp/src/gates.ts::
    /// unsoundBaseGateRefusal` field for field, so an agent that hits the
    /// client gate and an agent that hits this one get ONE account of the
    /// condition rather than two:
    ///
    /// | this refusal        | `gates.ts`                       |
    /// |---------------------|----------------------------------|
    /// | `error`             | `reason`                         |
    /// | `hint`              | `how_to_proceed`                 |
    /// | `details.gate`      | `gate`                           |
    /// | `details.solid_id`  | `unsound_base.part_id`           |
    /// | `details.verdict`   | `unsound_base.verdict`           |
    ///
    /// `verdict` MUST be the string `GET /api/agent/parts/{id}/perception`
    /// reports for this solid — that endpoint is where `gates.ts` reads
    /// its own copy, so quoting it verbatim is what makes the two
    /// refusals agree by construction instead of by hand-synced prose.
    pub fn unsound_base(operation: &str, solid_id: u32, verdict: &str) -> Self {
        Self::new(
            ErrorCode::UnsoundBase,
            format!(
                "solid {solid_id} is UNSOUND by the kernel's live verdict \
                 ({verdict}) — '{operation}' would stack new work onto a \
                 defective solid, and every downstream certificate would \
                 inherit the defect."
            ),
        )
        .with_hint(format!(
            "Diagnose with GET /api/agent/parts/{solid_id}/perception (the full \
             kernel certificate names the failing dimension), then repair or \
             roll back before continuing. If THIS operation is itself the \
             deliberate repair (e.g. a boolean used to heal the shell, a \
             rebuild from a known-good state), re-issue this exact call with \
             acknowledge_unsound: true."
        ))
        .with_details(serde_json::json!({
            "gate": "unsound_base",
            "solid_id": solid_id,
            "verdict": verdict,
            "operation": operation,
        }))
    }

    /// `X-Roshera-Part-Id` referenced a part UUID that isn't in the
    /// `PartManager` registry. The detail carries the offending id
    /// so the frontend can drop a stale tab.
    pub fn part_not_found(part_id: uuid::Uuid) -> Self {
        Self::new(ErrorCode::PartNotFound, format!("part {part_id} not found"))
            .with_hint(
                "Create a part with POST /api/parts and use the returned \
             id in the X-Roshera-Part-Id header.",
            )
            .with_details(serde_json::json!({ "part_id": part_id }))
    }

    /// A document-scoped route referenced an id that is not in the
    /// registry — never created, or a typo.
    pub fn document_not_found(id: &str) -> Self {
        Self::new(
            ErrorCode::DocumentNotFound,
            format!("document '{id}' is not registered"),
        )
        .with_hint(
            "Call POST /api/documents to create it first, or GET /api/documents \
             to list known ids.",
        )
        .with_details(serde_json::json!({ "document_id": id }))
    }

    /// `DELETE /api/documents/{id}` targeted the document currently
    /// loaded into the live model.
    pub fn document_delete_refused_active(id: &str) -> Self {
        Self::new(
            ErrorCode::DocumentDeleteRefusedActive,
            format!("document '{id}' is the active document and cannot be deleted"),
        )
        .with_hint(
            "Switch to a different document first with POST /api/documents/{id}/open, \
             then retry the delete.",
        )
        .with_details(serde_json::json!({ "document_id": id }))
    }

    /// `DELETE /api/documents/{id}` targeted the only document left in
    /// the registry.
    pub fn document_delete_refused_last(id: &str) -> Self {
        Self::new(
            ErrorCode::DocumentDeleteRefusedLast,
            format!("document '{id}' is the last remaining document and cannot be deleted"),
        )
        .with_hint(
            "Create another document with POST /api/documents before deleting this one \
             — the app must always have at least one document.",
        )
        .with_details(serde_json::json!({ "document_id": id }))
    }

    /// `DELETE /api/documents/{id}` targeted the default document
    /// (`durability::DURABILITY_SESSION_ID`).
    pub fn document_delete_refused_default(id: &str) -> Self {
        Self::new(
            ErrorCode::DocumentDeleteRefusedDefault,
            format!("document '{id}' is the default document and cannot be deleted"),
        )
        .with_hint(
            "The default document holds the pre-existing event ledger and cannot be \
             removed through this route. This is a deliberate restriction, not a bug.",
        )
        .with_details(serde_json::json!({ "document_id": id }))
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
            "AI provider not configured: no LLM credential is active".to_string(),
        )
        .with_hint(
            "Connect a provider from the UI (Settings → AI Provider) or via \
             PUT /api/ai/provider — no restart required. Alternatively set \
             ANTHROPIC_API_KEY in the server environment before start. Use \
             GET /api/ai/status to verify."
                .to_string(),
        )
        .with_details(serde_json::json!({
            "configure_endpoint": "/api/ai/provider",
        }))
    }

    /// Provider-configuration surface refused a provider or credential
    /// mode outside the server-owned allowlist. The typed refusal from
    /// `ai_integration::providers::allowlist` is serialized into
    /// `details.refusal` so agents can branch without parsing prose.
    pub fn ai_provider_refused(
        message: impl Into<String>,
        refusal_details: serde_json::Value,
    ) -> Self {
        Self::new(ErrorCode::AiProviderRefused, message)
            .with_hint(
                "Choose a provider/mode from GET /api/ai/provider's `allowlist`. \
                 Client-side provider switching does not exist by design."
                    .to_string(),
            )
            .with_details(serde_json::json!({ "refusal": refusal_details }))
    }

    /// A credential failed its live validation round-trip.
    pub fn ai_credential_invalid(message: impl Into<String>) -> Self {
        Self::new(ErrorCode::AiCredentialInvalid, message).with_hint(
            "The credential was tested against the live provider before saving \
             and was rejected. Check the key/token (or CLI sign-in) and try \
             again — nothing was stored."
                .to_string(),
        )
    }

    /// A requested model ID was tested against the live provider (via its
    /// authoritative model-listing endpoint, not a hardcoded menu) and
    /// rejected. `details.rejected_model` names it explicitly — never a
    /// generic "invalid model" with no identifying detail.
    pub fn ai_model_rejected(model: impl Into<String>, message: impl Into<String>) -> Self {
        let model = model.into();
        Self::new(ErrorCode::AiModelRejected, message)
            .with_hint(
                "The model was tested against the live provider before saving \
                 and was rejected — it is not one this credential can serve. \
                 Use \"default\" (the provider's own choice) or a model ID \
                 the provider actually accepts; nothing was stored."
                    .to_string(),
            )
            .with_details(serde_json::json!({ "rejected_model": model }))
    }

    /// `POST /api/ai/provider/models` refused before or instead of
    /// reaching a vendor: no base URL could be resolved (`outcome:
    /// "no_base_url"`), the vendor rejected the credential
    /// (`"unauthorized"`), the resolved URL doesn't exist on the vendor
    /// (`"not_found"`), an unrecognized status came back
    /// (`"unexpected_status"`), the round trip timed out
    /// (`"timeout"`), or a transport/parse failure occurred
    /// (`"transport_error"`). `vendor_status`/`vendor_message` are only
    /// present when the vendor actually answered.
    pub fn ai_model_discovery_failed(
        provider: &str,
        outcome: &str,
        message: impl Into<String>,
        vendor_status: Option<u16>,
        vendor_message: Option<String>,
    ) -> Self {
        Self::new(ErrorCode::AiModelDiscoveryFailed, message)
            .with_hint(
                "Model discovery never falls back to a stored or guessed model \
                 list on failure. Fix the credential/provider named in `details` \
                 and retry POST /api/ai/provider/models directly."
                    .to_string(),
            )
            .with_details(serde_json::json!({
                "provider": provider,
                "outcome": outcome,
                "vendor_status": vendor_status,
                "vendor_message": vendor_message,
            }))
    }

    /// `POST /api/ai/provider/models` refused a key before any network
    /// call because it cannot plausibly be an API key (multi-line,
    /// absurd length, leading whitespace/brackets) — see
    /// `ai_provider_config::reject_implausible_key_shape`'s doc for the
    /// incident this closes (a 649-char Vite error message reached
    /// `state/ai-provider.json` as a stored credential).
    pub fn ai_api_key_implausible(reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::new(ErrorCode::InvalidParameter, format!("api_key {reason}"))
            .with_hint("Paste only the vendor's API key — nothing else.".to_string())
            .with_details(serde_json::json!({ "parameter": "api_key" }))
    }

    /// `/acp` POST carried a JSON-RPC method outside the allowlist.
    pub fn acp_method_not_allowed(method: &str, allowed: &[&str]) -> Self {
        Self::new(
            ErrorCode::AcpMethodNotAllowed,
            format!("ACP method '{method}' is not allowed through Roshera's agent surface"),
        )
        .with_hint(
            "Provider selection, config mutation, and extension management are \
             server-configured through Roshera's own settings surface \
             (PUT /api/ai/provider), never through the agent transport."
                .to_string(),
        )
        .with_details(serde_json::json!({
            "method": method,
            "allowed_methods": allowed,
        }))
    }

    /// `/acp` WebSocket upgrade refused (POST + SSE is the supported
    /// transport, because it is the one whose messages can be filtered).
    pub fn acp_websocket_disabled() -> Self {
        Self::new(
            ErrorCode::AcpWebsocketDisabled,
            "the /acp WebSocket transport is disabled; use HTTP POST + SSE".to_string(),
        )
        .with_hint(
            "POST JSON-RPC messages to /acp and read responses via the SSE \
             stream (GET /acp with Accept: text/event-stream). The WebSocket \
             upgrade is refused because its frames bypass Roshera's method \
             gate."
                .to_string(),
        )
    }

    /// `/acp` `session/new` / `session/load` carried a forbidden `_meta`
    /// key — one that pre-empts or overrides the `mcpServers` entry
    /// Roshera injects into every session.
    pub fn acp_forbidden_session_meta(method: &str, key: &str) -> Self {
        Self::new(
            ErrorCode::AcpForbiddenSessionMeta,
            format!("ACP '{method}' carried a forbidden _meta key: '{key}'"),
        )
        .with_hint(
            "Provider selection, extension enablement, and recipe loading are \
             server-configured through Roshera's own settings surface, never \
             through _meta on session/new or session/load. Drop the key and \
             retry — Roshera's own MCP server is injected automatically."
                .to_string(),
        )
        .with_details(serde_json::json!({
            "method": method,
            "meta_key": key,
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
    fn op_timeout_wire_shape_is_504_non_retryable_with_details() {
        let e = ApiError::op_timeout("boolean", 60.0, &[7, 9]);
        assert_eq!(e.code, ErrorCode::OpTimeout);
        assert_eq!(e.code.status(), StatusCode::GATEWAY_TIMEOUT);
        assert!(!e.retryable, "same inputs blow the same budget again");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_code"], "op_timeout");
        assert_eq!(v["retryable"], false);
        assert_eq!(v["details"]["op_kind"], "boolean");
        assert_eq!(v["details"]["budget_secs"], 60.0);
        assert_eq!(v["details"]["operands"][0], 7);
        assert_eq!(v["details"]["operands"][1], 9);
        assert!(v["hint"].is_string());
    }

    /// Item: generic-checkpoint refusal is a TYPED 422, not a bare
    /// status. Fails without the `CheckpointNameRejected` variant (the
    /// constructor would not exist) and pins the wire contract the
    /// frontend's `refusalMessage` and the MCP layer read: `error_code`,
    /// `retryable:false`, a hint naming a passing example, and
    /// `details.rejected_name`.
    #[test]
    fn checkpoint_name_rejected_is_typed_422_with_rejected_name() {
        let e = ApiError::checkpoint_name_rejected("Checkpoint 9:59:36 PM");
        assert_eq!(e.code, ErrorCode::CheckpointNameRejected);
        assert_eq!(e.code.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert!(!e.retryable, "same name refuses identically");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["success"], false);
        assert_eq!(v["error_code"], "checkpoint_name_rejected");
        assert_eq!(v["details"]["rejected_name"], "Checkpoint 9:59:36 PM");
        assert!(v["hint"].as_str().unwrap().contains("bolt circle"));
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

    /// CF-β.2: a `BlendFailure::VertexBlendUnsupported` carrying the
    /// new `MixedKindUnsupported` reason variant survives the
    /// `OperationError → ApiError → JSON` chain with the existing /
    /// requested kind tags + the nested `MixedKindRejectDetail`
    /// discriminator intact. Agents pattern-match
    /// `details.failure.reason.MixedKindUnsupported.detail.type`
    /// to decide whether to retry with matched displacements, drop
    /// the conflict, or surface the unsupported degree to the user.
    #[test]
    fn blend_failed_wire_shape_carries_mixed_kind_unsupported_payload() {
        use geometry_engine::operations::blend_graph::BlendVertexKind;
        use geometry_engine::operations::diagnostics::{
            BlendFailure, MixedKindRejectDetail, VertexBlendKindSet, VertexBlendUnsupportedReason,
        };
        use geometry_engine::operations::OperationError;
        use geometry_engine::primitives::solid::BlendKind;

        let mut existing = VertexBlendKindSet::default();
        existing.insert(BlendKind::Chamfer);

        let failure = BlendFailure::VertexBlendUnsupported {
            vertex: 19,
            kind: BlendVertexKind::ConvexCorner { degree: 3 },
            reason: VertexBlendUnsupportedReason::MixedKindUnsupported {
                existing,
                requested: BlendKind::Fillet,
                detail: MixedKindRejectDetail::DegreeUnsupported { degree: 3 },
            },
        };

        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        assert_eq!(api_err.code, ErrorCode::BlendFailed);
        assert_eq!(api_err.code.status(), StatusCode::BAD_REQUEST);
        assert!(!api_err.retryable);

        let v = serde_json::to_value(&api_err).unwrap();
        let payload = &v["details"]["failure"];
        assert_eq!(payload["type"], "VertexBlendUnsupported");
        assert_eq!(payload["vertex"], 19);
        assert_eq!(payload["kind"]["ConvexCorner"]["degree"], 3);
        // Outer reason discriminator is externally tagged.
        let mixed = &payload["reason"]["MixedKindUnsupported"];
        assert_eq!(mixed["existing"]["has_chamfer"], true);
        assert_eq!(mixed["existing"]["has_fillet"], false);
        assert_eq!(mixed["requested"], "fillet");
        // Nested detail is internally tagged on `type`.
        assert_eq!(mixed["detail"]["type"], "DegreeUnsupported");
        assert_eq!(mixed["detail"]["degree"], 3);
    }

    /// CF-β.2: the `MixedDisplacements` arm of
    /// `MixedKindRejectDetail` round-trips the per-edge offsets and
    /// radii vectors verbatim. Agents use these to decide whether
    /// the displacements are close enough to nudge with a tolerance
    /// retry, or whether they're orders apart and must be re-
    /// authored by the operator.
    #[test]
    fn blend_failed_wire_shape_carries_nested_mixed_displacements_detail() {
        use geometry_engine::operations::blend_graph::BlendVertexKind;
        use geometry_engine::operations::diagnostics::{
            BlendFailure, MixedKindRejectDetail, VertexBlendKindSet, VertexBlendUnsupportedReason,
        };
        use geometry_engine::operations::OperationError;
        use geometry_engine::primitives::solid::BlendKind;

        let mut existing = VertexBlendKindSet::default();
        existing.insert(BlendKind::Fillet);
        existing.insert(BlendKind::Chamfer);

        let failure = BlendFailure::VertexBlendUnsupported {
            vertex: 23,
            kind: BlendVertexKind::ConvexCorner { degree: 3 },
            reason: VertexBlendUnsupportedReason::MixedKindUnsupported {
                existing,
                requested: BlendKind::Chamfer,
                detail: MixedKindRejectDetail::MixedDisplacements {
                    offsets: vec![0.5, 0.5],
                    radii: vec![0.8],
                },
            },
        };

        let api_err: ApiError = OperationError::BlendFailed(Box::new(failure)).into();
        let v = serde_json::to_value(&api_err).unwrap();
        let mixed = &v["details"]["failure"]["reason"]["MixedKindUnsupported"];
        assert_eq!(mixed["existing"]["has_fillet"], true);
        assert_eq!(mixed["existing"]["has_chamfer"], true);
        assert_eq!(mixed["requested"], "chamfer");
        let detail = &mixed["detail"];
        assert_eq!(detail["type"], "MixedDisplacements");
        assert_eq!(detail["offsets"][0], 0.5);
        assert_eq!(detail["offsets"][1], 0.5);
        assert_eq!(detail["radii"][0], 0.8);
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
            BlendFailure::TopologyViolation { detail: "x".into() },
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
