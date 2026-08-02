//! Provider-epoch invalidation for the `/acp` surface.
//!
//! ## The defect this forecloses
//!
//! goose stores the provider ON THE SESSION and restores it
//! (`goose::execution::manager: Restoring evicted session … (provider:
//! Some("sarvam"))` — observed live). A session created while one provider
//! was pinned keeps that provider for its whole life, so repinning via
//! `PUT /api/ai/provider` changed the *default* for future sessions while
//! the browser tab kept prompting the old one indefinitely. The settings
//! dialog truthfully reported the pin succeeded; the thing serving turns
//! was unchanged. The only cure was knowing to reload the page.
//!
//! ## The mechanism
//!
//! Every ACP connection is stamped with the provider epoch current when
//! goose minted its `Acp-Connection-Id` (the `initialize` response — the
//! only response that carries the header without the request having sent
//! it). A successful repin bumps the epoch. Any subsequent request bearing
//! a connection id from an older epoch is refused with a bare `404` and an
//! empty body — byte-identical to what goose's own transport returns for a
//! connection id it does not know (`agent-client-protocol-http`'s
//! `handle_post`/`handle_get`/`handle_delete`, all
//! `StatusCode::NOT_FOUND.into_response()`), which is exactly the
//! backend-restart signature `roshera-app`'s `acp-client.ts` already
//! recovers from by re-running `initialize` + `session/new`. The stale
//! state is made IMPOSSIBLE to keep serving, rather than explained in a
//! doc nobody reads: the next prompt structurally cannot reach the old
//! session, and the fresh `session/new` reads goose's new active provider.
//!
//! ## In-flight turns — a deliberate choice
//!
//! A turn already streaming when the epoch bumps is allowed to FINISH on
//! the provider that started it; the invalidation takes effect at the next
//! request on that connection. Chosen deliberately over mid-turn
//! cancellation: the turn's tool calls are already landing on the timeline
//! (killing it halfway risks a half-applied geometry sequence with no
//! turn-end bookkeeping), the tokens are already spent, and the turn's
//! results are honestly attributed to the provider that actually produced
//! them. A session can only change provider at a session boundary anyway —
//! so the turn boundary is the earliest boundary at which the switch can
//! mean anything.

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// The header goose's ACP HTTP transport uses to address a connection —
/// lowercase, matching how axum normalizes inbound header names.
pub(crate) const ACP_CONNECTION_ID_HEADER: &str = "acp-connection-id";

/// Monotone provider epoch + the epoch each live ACP connection was minted
/// under. Shared between the `/acp` middleware ([`enforce_provider_epoch`])
/// and `PUT /api/ai/provider` ([`AcpProviderEpoch::invalidate_connections`]).
#[derive(Debug, Default)]
pub(crate) struct AcpProviderEpoch {
    current: AtomicU64,
    /// connection id → epoch at mint time. Entries are kept after a bump
    /// ON PURPOSE: a retained stale entry is what lets [`is_stale`] refuse
    /// the id by fact rather than by absence.
    connections: DashMap<String, u64>,
}

impl AcpProviderEpoch {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn current(&self) -> u64 {
        self.current.load(Ordering::SeqCst)
    }

    /// Bump the epoch: every connection minted before this call becomes
    /// stale and will be refused with the 404 recovery signature. Called
    /// by `PUT /api/ai/provider` after EVERY successful repin branch —
    /// subscription CLI, declarative vendor, and the anthropic default —
    /// never on a failed or refused save. Returns the new epoch (for the
    /// caller's log line).
    pub(crate) fn invalidate_connections(&self) -> u64 {
        self.current.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Stamp a freshly-minted connection with the current epoch.
    /// `pub(crate)` for the handler-level test in `handlers/ai_provider.rs`
    /// — production registration happens only in [`enforce_provider_epoch`].
    pub(crate) fn register(&self, connection_id: &str) {
        self.connections
            .insert(connection_id.to_string(), self.current());
    }

    /// `true` only for a connection this process registered under an
    /// OLDER epoch. An id we never saw is NOT stale — it passes through
    /// to goose, which answers 404 itself for ids it does not know; this
    /// middleware never invents a verdict about a connection it holds no
    /// fact about. (`pub(crate)` for the same handler-level test as
    /// [`Self::register`].)
    pub(crate) fn is_stale(&self, connection_id: &str) -> bool {
        self.connections
            .get(connection_id)
            .map(|entry| *entry.value() < self.current())
            .unwrap_or(false)
    }
}

/// Axum middleware layered around the `/acp` router (outermost of its
/// inner layers, so a stale connection is refused before turn bookkeeping
/// or MCP injection ever see the request).
///
/// - Request carries a connection id minted under an older epoch → bare
///   `404`, empty body: the exact signature `acp-client.ts` already treats
///   as "connection no longer exists, reestablish".
/// - Request carries a current (or unknown-to-us) connection id → pass
///   through untouched.
/// - Request carries no connection id (`initialize`, CORS preflight) →
///   pass through; if the RESPONSE carries `Acp-Connection-Id` (goose just
///   minted a connection), register it under the current epoch.
pub(crate) async fn enforce_provider_epoch(
    State(epoch): State<Arc<AcpProviderEpoch>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let request_connection_id = req
        .headers()
        .get(ACP_CONNECTION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    match request_connection_id {
        Some(id) if epoch.is_stale(&id) => {
            tracing::info!(
                target: "goose_acp",
                connection_id = %id,
                "refusing ACP connection minted under a previous provider \
                 epoch — the client's 404 recovery path starts a fresh \
                 session on the newly pinned provider"
            );
            StatusCode::NOT_FOUND.into_response()
        }
        Some(_) => next.run(req).await,
        None => {
            let response = next.run(req).await;
            if let Some(id) = response
                .headers()
                .get(ACP_CONNECTION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                epoch.register(id);
            }
            response
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::routing::any;
    use axum::Router;
    use tower::ServiceExt;

    /// Stub standing in for goose's transport: a POST WITHOUT a
    /// connection-id header behaves like `initialize` (mints an id via the
    /// response header — the id is taken from the request's
    /// `x-test-mint-id` so each test controls it); anything else echoes
    /// 200. This isolates the middleware's contract from goose itself.
    fn stub_acp_router(epoch: Arc<AcpProviderEpoch>) -> Router {
        let handler = |req: Request<Body>| async move {
            let mut response = StatusCode::OK.into_response();
            if req.headers().get(ACP_CONNECTION_ID_HEADER).is_none() {
                if let Some(mint) = req.headers().get("x-test-mint-id") {
                    response
                        .headers_mut()
                        .insert(ACP_CONNECTION_ID_HEADER, mint.clone());
                }
            }
            response
        };
        Router::new()
            .route("/acp", any(handler))
            .layer(axum::middleware::from_fn_with_state(
                epoch,
                enforce_provider_epoch,
            ))
    }

    async fn send(
        router: &Router,
        method: &str,
        connection_id: Option<&str>,
        mint_id: Option<&str>,
    ) -> (StatusCode, axum::body::Bytes) {
        let mut builder = Request::builder().method(method).uri("/acp");
        if let Some(id) = connection_id {
            builder = builder.header(ACP_CONNECTION_ID_HEADER, id);
        }
        if let Some(id) = mint_id {
            builder = builder.header("x-test-mint-id", id);
        }
        let response = router
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = response.status();
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        (status, body)
    }

    #[tokio::test]
    async fn provider_repin_invalidates_connections_with_the_exact_404_recovery_signature() {
        let epoch = Arc::new(AcpProviderEpoch::new());
        let router = stub_acp_router(epoch.clone());

        // initialize: no connection header, response mints one — the
        // middleware must register it under the current epoch.
        let (status, _) = send(&router, "POST", None, Some("conn-old")).await;
        assert_eq!(status, StatusCode::OK);

        // The connection serves normally before any repin.
        let (status, _) = send(&router, "POST", Some("conn-old"), None).await;
        assert_eq!(status, StatusCode::OK, "pre-repin request must pass");

        // The repin: PUT /api/ai/provider's success path calls this.
        epoch.invalidate_connections();

        // POST and GET (SSE) on the stale connection are both refused with
        // a bare 404 and an EMPTY body — the exact shape
        // agent-client-protocol-http gives an unknown id and the exact
        // shape acp-client.ts's reestablish path branches on.
        for method in ["POST", "GET", "DELETE"] {
            let (status, body) = send(&router, method, Some("conn-old"), None).await;
            assert_eq!(
                status,
                StatusCode::NOT_FOUND,
                "{method} on a stale connection must 404 so the client reestablishes"
            );
            assert!(
                body.is_empty(),
                "{method}: the 404 body must be empty — acp-client.ts matches the \
                 backend-restart signature, not an error envelope"
            );
        }

        // Recovery: a fresh initialize passes and its new connection —
        // minted under the NEW epoch — serves.
        let (status, _) = send(&router, "POST", None, Some("conn-new")).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "fresh initialize must pass after a repin"
        );
        let (status, _) = send(&router, "POST", Some("conn-new"), None).await;
        assert_eq!(
            status,
            StatusCode::OK,
            "a connection minted after the repin must serve — invalidation is \
             one-shot per epoch, never a permanent lockout"
        );
    }

    #[tokio::test]
    async fn unknown_connection_ids_pass_through_for_goose_to_judge() {
        let epoch = Arc::new(AcpProviderEpoch::new());
        let router = stub_acp_router(epoch.clone());
        epoch.invalidate_connections();

        // An id this process never registered is NOT this middleware's to
        // refuse — goose's own transport 404s ids it does not know, and a
        // verdict here would be a claim with no fact behind it.
        let (status, _) = send(&router, "POST", Some("never-registered"), None).await;
        assert_eq!(status, StatusCode::OK);
    }

    #[tokio::test]
    async fn repin_before_any_connection_exists_is_harmless() {
        let epoch = Arc::new(AcpProviderEpoch::new());
        let router = stub_acp_router(epoch.clone());

        epoch.invalidate_connections();
        let (status, _) = send(&router, "POST", None, Some("conn-a")).await;
        assert_eq!(status, StatusCode::OK);
        let (status, _) = send(&router, "POST", Some("conn-a"), None).await;
        assert_eq!(status, StatusCode::OK);
    }
}
