//! Method gate for the `/acp` (goose agent) surface — closes the
//! client-side provider-switching hole at Roshera's own layer.
//!
//! # Threat
//!
//! goose's ACP server dispatches JSON-RPC methods that mutate
//! server-side configuration when invoked by a *client*:
//!
//! - `session/set_config_option` with `config_id: "provider"` (and
//!   `providers/set` / `providers/disable`) resolve a client-supplied
//!   provider name against a registry populated unconditionally at
//!   startup — including subprocess-bridge providers (`claude-acp`,
//!   `claude-code`, `codex`, `cursor-agent`, …) that spawn CLI binaries
//!   present on the host, plus local-inference backends.
//! - The `_goose/*` custom-method family includes `ConfigUpsert`,
//!   `SetConfigExtensionEnabled`, `AddConfigExtension`,
//!   `ProviderConfigSave`, `LocalInference*`, and scheduler verbs — any
//!   of which could re-open tool surface or provider choice at runtime,
//!   silently outliving the boot-time lockdown in `goose_acp.rs`.
//!
//! `AcpServerFactoryConfig` offers no capability flag to disable any of
//! this, so the gate lives here, on the axum router Roshera owns.
//!
//! # Design: default-deny on an explicit method allowlist
//!
//! Rather than blocklisting `session/set_config_option` (and being
//! wrong the day upstream adds a new mutating verb), the gate allows
//! exactly the methods an agent conversation needs and refuses
//! everything else with a typed error. A goose dependency bump that
//! introduces new RPCs changes nothing here — new methods are refused
//! until deliberately added.
//!
//! # Transports — both closed, honestly
//!
//! The upstream ACP HTTP server (`agent-client-protocol-http`) exposes
//! three transports on `/acp`:
//!
//! - **HTTP POST** — one JSON-RPC message per request body (batches get
//!   501 upstream; refused here too). Filterable, and filtered.
//! - **SSE** (GET with `Accept: text/event-stream`) — server→client
//!   only; carries no client-initiated methods. Passed through.
//! - **WebSocket** (GET with upgrade headers) — bidirectional frames
//!   whose loop goose owns; a middleware cannot inspect them. Instead
//!   of claiming a filter that cannot be enforced, the upgrade itself
//!   is REFUSED wholesale, forcing clients onto POST + SSE where every
//!   inbound message passes this gate. This is what makes the
//!   `update_provider` closure true on *both* transports rather than
//!   HTTP-only.
//!
//! Client→agent *responses* (JSON-RPC messages without a `method`
//! field, e.g. replies to `session/request_permission`) pass through:
//! they cannot invoke anything.
//!
//! # Body inspection on `session/new` / `session/load`
//!
//! The method allowlist alone is not enough for these two methods:
//! their *params* carry a `_meta` object, and four keys on it are each
//! an arbitrary-command surface in their own right, independent of the
//! `mcpServers` field:
//!
//! - `_meta.provider` — overrides the resolved provider
//!   (`resolve_provider_and_model`, `acp/server/new_session.rs`),
//!   reaching the same subprocess-bridge provider registry documented
//!   on `goose_acp::acp_router`.
//! - `_meta.enabledExtensions` — decoded as goose's own
//!   `GooseExtension` list and takes priority over `mcpServers` in
//!   `initial_session_extensions` (`acp/server.rs`): a client that
//!   sets this routes *around* the `mcpServers` rewrite entirely,
//!   because the `mcpServers`-only branch is only reached when no
//!   `goose_extensions` were supplied.
//! - `_meta.recipeDeeplink` / `_meta.recipeId` — resolved into a full
//!   `Recipe` (`resolve_recipe_from_meta`, `acp/server/recipe/mod.rs`),
//!   which can itself carry a `provider`/`model` pin and its own
//!   `extensions` list — a second, independent path to both hazards
//!   above.
//!
//! `acp_method_gate` therefore parses the body of `session/new` and
//! `session/load` POSTs (already buffered for the method check) and
//! refuses the request outright — `acp_forbidden_session_meta`, HTTP
//! 403 — if any of those four keys is present under `params._meta`.
//! Refused, not stripped: a client sending one of these is not a
//! well-behaved caller of Roshera's intended flow, and a silent strip
//! would let it believe the override took effect. This runs *before*
//! the `mcpServers`-rewriting middleware layered inside
//! `goose_acp::acp_router` (this gate wraps that router at the merge
//! site in `main.rs`), so a forbidden body never reaches the point
//! where a per-session key would be minted.

use axum::{
    body::Body,
    http::{header, HeaderValue, Method, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::error_catalog::ApiError;

/// The complete set of client-initiated JSON-RPC methods allowed
/// through `/acp`. Everything an agent chat needs; nothing that
/// mutates provider, config, extension, or scheduler state.
///
/// Extend deliberately: adding a method here is a security-review
/// decision, not a convenience edit. In particular, keep out:
/// `session/set_config_option`, `session/set_mode`, `providers/*`,
/// `session/fork`, `session/delete`, `logout`, and the entire
/// `_goose/*` custom family.
pub(crate) const ALLOWED_ACP_METHODS: &[&str] = &[
    "initialize",
    "authenticate",
    "session/new",
    "session/load",
    "session/list",
    "session/prompt",
    // Cancellation notifications — both spellings the transport uses.
    "session/cancel",
    "$/cancel_request",
];

/// Body cap for gate buffering. Matches the upstream transport's own
/// POST cap so the gate never rejects a body upstream would accept.
const MAX_ACP_POST_BYTES: usize = 16 * 1024 * 1024;

/// `_meta` keys on `session/new` / `session/load` that are refused
/// outright. See the module doc "Body inspection" section for why each
/// one is a hazard independent of the method allowlist and independent
/// of the `mcpServers` rewrite in `goose_acp::acp_router`.
const FORBIDDEN_SESSION_META_KEYS: &[&str] = &[
    "provider",
    "enabledExtensions",
    "recipeDeeplink",
    "recipeId",
];

/// The two methods whose body carries a `_meta` object worth
/// inspecting. Every other allowed method (`initialize`, `authenticate`,
/// `session/list`, `session/prompt`, cancellation) either takes no
/// `_meta` capable of this class of override or is not session-
/// establishing.
const SESSION_META_INSPECTED_METHODS: &[&str] = &["session/new", "session/load"];

/// Returns the first forbidden `_meta` key present under
/// `message.params._meta`, if any. `None` for a missing/malformed
/// `params`/`_meta` — those shapes carry no override to refuse, and a
/// malformed body is upstream's own 400 to raise, not this gate's.
fn forbidden_session_meta_key(message: &serde_json::Value) -> Option<&'static str> {
    let meta = message.get("params")?.get("_meta")?.as_object()?;
    FORBIDDEN_SESSION_META_KEYS
        .iter()
        .copied()
        .find(|key| meta.contains_key(*key))
}

fn header_contains_token(value: Option<&HeaderValue>, token: &str) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case(token))
        })
}

fn is_websocket_upgrade(req: &Request<Body>) -> bool {
    req.method() == Method::GET
        && header_contains_token(req.headers().get(header::CONNECTION), "upgrade")
        && req
            .headers()
            .get(header::UPGRADE)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Axum middleware enforcing the ACP method allowlist. Applied to the
/// `/acp` router only (see the merge site in `main.rs`), inside the
/// global auth stack — authentication happens first, then this gate.
pub(crate) async fn acp_method_gate(req: Request<Body>, next: Next) -> Response {
    if is_websocket_upgrade(&req) {
        return ApiError::acp_websocket_disabled().into_response();
    }

    if req.method() != Method::POST {
        // GET (SSE subscribe) and DELETE (connection teardown) carry no
        // client-initiated JSON-RPC methods.
        return next.run(req).await;
    }

    let (parts, body) = req.into_parts();
    let bytes = match axum::body::to_bytes(body, MAX_ACP_POST_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (StatusCode::PAYLOAD_TOO_LARGE, "POST body too large").into_response();
        }
    };

    // Batches are refused outright: upstream 501s them anyway, and a
    // gate that only inspected the first element would be a smuggling
    // vector if upstream ever started accepting arrays.
    if bytes.first() == Some(&b'[') {
        return ApiError::acp_method_not_allowed("<json-rpc batch>", ALLOWED_ACP_METHODS)
            .into_response();
    }

    if let Ok(message) = serde_json::from_slice::<serde_json::Value>(&bytes) {
        if let Some(method) = message.get("method").and_then(|m| m.as_str()) {
            if !ALLOWED_ACP_METHODS.contains(&method) {
                return ApiError::acp_method_not_allowed(method, ALLOWED_ACP_METHODS)
                    .into_response();
            }
            if SESSION_META_INSPECTED_METHODS.contains(&method) {
                if let Some(key) = forbidden_session_meta_key(&message) {
                    return ApiError::acp_forbidden_session_meta(method, key).into_response();
                }
            }
        }
        // No `method` field: a client→agent response — cannot invoke
        // anything; pass through.
    }
    // Unparseable JSON passes through: the upstream transport parses
    // with the same serde_json and returns its own 400. Nothing
    // unparseable can dispatch a method.

    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::routing::{delete, get, post};
    use axum::Router;
    use tower::ServiceExt;

    /// A stand-in for goose's `/acp` router: accepts everything, like
    /// the real transport does (202 on POST, 200 on GET, 202 DELETE).
    /// The gate under test wraps it exactly as `main.rs` wraps the
    /// real one.
    fn gated_stub_router() -> Router {
        Router::new()
            .route("/acp", post(|| async { StatusCode::ACCEPTED }))
            .route("/acp", get(|| async { (StatusCode::OK, "sse ok") }))
            .route("/acp", delete(|| async { StatusCode::ACCEPTED }))
            .layer(axum::middleware::from_fn(acp_method_gate))
    }

    fn rpc_post(method: &str) -> Request<Body> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": {}
        })
        .to_string();
        Request::builder()
            .method("POST")
            .uri("/acp")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    async fn status_and_body(req: Request<Body>) -> (StatusCode, serde_json::Value) {
        let response = gated_stub_router().oneshot(req).await.unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
        (status, value)
    }

    /// THE proving test of the transport gate: every provider-switching
    /// and config-mutating RPC goose dispatches must be refused with
    /// the typed `acp_method_not_allowed` error — never forwarded.
    #[tokio::test]
    async fn provider_switch_and_config_methods_are_refused() {
        for method in [
            // The `session/update_provider` surface (SACP spelling).
            "session/set_config_option",
            "providers/set",
            "providers/disable",
            "providers/list",
            // Mode changes gate tool auto-approval — server-side only.
            "session/set_mode",
            // Config / extension mutation via the custom family.
            "_goose/config_upsert",
            "_goose/set_config_extension_enabled",
            "_goose/provider_config_save",
            "_goose/local_inference_model_download",
            // Anything unknown is refused by default-deny.
            "some/future_method",
        ] {
            let (status, body) = status_and_body(rpc_post(method)).await;
            assert_eq!(
                status,
                StatusCode::FORBIDDEN,
                "method '{method}' must be refused, got {status}"
            );
            assert_eq!(
                body["error_code"], "acp_method_not_allowed",
                "method '{method}' must carry the typed code; body: {body}"
            );
            assert_eq!(body["details"]["method"], method);
        }
    }

    fn rpc_post_with_session_meta(method: &str, meta: serde_json::Value) -> Request<Body> {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": {
                "cwd": "/tmp/session",
                "mcpServers": [],
                "_meta": meta
            }
        })
        .to_string();
        Request::builder()
            .method("POST")
            .uri("/acp")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    /// THE proving test of the body gate: each `_meta` key that can
    /// pre-empt or override the injected `mcpServers` entry must be
    /// refused with the typed `acp_forbidden_session_meta` error, on
    /// both session-establishing methods.
    #[tokio::test]
    async fn forbidden_session_meta_keys_are_refused() {
        for method in ["session/new", "session/load"] {
            for (key, value) in [
                ("provider", serde_json::json!("openai")),
                (
                    "enabledExtensions",
                    serde_json::json!([{"name": "developer"}]),
                ),
                ("recipeDeeplink", serde_json::json!("goose://recipe?data=x")),
                ("recipeId", serde_json::json!("some-recipe-id")),
            ] {
                let meta = serde_json::json!({ key: value });
                let (status, body) =
                    status_and_body(rpc_post_with_session_meta(method, meta)).await;
                assert_eq!(
                    status,
                    StatusCode::FORBIDDEN,
                    "'{method}' with _meta.{key} must be refused, got {status}"
                );
                assert_eq!(
                    body["error_code"], "acp_forbidden_session_meta",
                    "'{method}' with _meta.{key} must carry the typed code; body: {body}"
                );
                assert_eq!(body["details"]["method"], method);
                assert_eq!(body["details"]["meta_key"], key);
            }
        }
    }

    /// A `session/new` / `session/load` body with an unrelated `_meta`
    /// key (or none at all) is not this gate's concern — it must reach
    /// the inner router exactly like any other allowed method.
    #[tokio::test]
    async fn benign_session_meta_passes_through() {
        for method in ["session/new", "session/load"] {
            let meta = serde_json::json!({ "hidden": true, "client": "roshera" });
            let (status, _) = status_and_body(rpc_post_with_session_meta(method, meta)).await;
            assert_eq!(
                status,
                StatusCode::ACCEPTED,
                "'{method}' with benign _meta must reach the inner router"
            );
        }
    }

    #[tokio::test]
    async fn conversation_methods_pass_through() {
        for method in ALLOWED_ACP_METHODS {
            let (status, _) = status_and_body(rpc_post(method)).await;
            assert_eq!(
                status,
                StatusCode::ACCEPTED,
                "allowed method '{method}' must reach the inner router"
            );
        }
    }

    /// Client→agent responses (no `method` field) answer agent-initiated
    /// requests like `session/request_permission`; they cannot invoke
    /// anything and must pass.
    #[tokio::test]
    async fn json_rpc_responses_pass_through() {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": { "outcome": { "outcome": "selected", "optionId": "allow" } }
        })
        .to_string();
        let req = Request::builder()
            .method("POST")
            .uri("/acp")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        let (status, _) = status_and_body(req).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn batches_are_refused() {
        let req = Request::builder()
            .method("POST")
            .uri("/acp")
            .header("content-type", "application/json")
            .body(Body::from(
                r#"[{"jsonrpc":"2.0","id":1,"method":"session/prompt","params":{}}]"#,
            ))
            .unwrap();
        let (status, body) = status_and_body(req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error_code"], "acp_method_not_allowed");
    }

    /// The WebSocket transport is refused wholesale — goose owns its
    /// frame loop, so the only honest enforcement is to keep it from
    /// existing. This is what closes provider switching on the second
    /// transport rather than HTTP-only.
    #[tokio::test]
    async fn websocket_upgrade_is_refused() {
        let req = Request::builder()
            .method("GET")
            .uri("/acp")
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "websocket")
            .header(header::SEC_WEBSOCKET_KEY, "dGhlIHNhbXBsZSBub25jZQ==")
            .header(header::SEC_WEBSOCKET_VERSION, "13")
            .body(Body::empty())
            .unwrap();
        let (status, body) = status_and_body(req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(body["error_code"], "acp_websocket_disabled");
    }

    /// Plain SSE subscription (GET without upgrade headers) is the
    /// server→client half of the supported transport and must pass.
    #[tokio::test]
    async fn sse_get_passes_through() {
        let req = Request::builder()
            .method("GET")
            .uri("/acp")
            .header(header::ACCEPT, "text/event-stream")
            .body(Body::empty())
            .unwrap();
        let (status, _) = status_and_body(req).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn delete_passes_through() {
        let req = Request::builder()
            .method("DELETE")
            .uri("/acp")
            .body(Body::empty())
            .unwrap();
        let (status, _) = status_and_body(req).await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }
}
