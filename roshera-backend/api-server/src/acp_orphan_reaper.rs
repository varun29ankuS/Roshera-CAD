//! Reaping of abandoned `/acp` connections.
//!
//! ## The defect this forecloses
//!
//! goose's ACP HTTP transport (`agent-client-protocol-http`) keeps a
//! connection, and with it the connection's `GooseAcpAgent`, every loaded
//! session agent and every stdio MCP extension process those sessions
//! spawned (one `node roshera-mcp/dist/index.js` per session), until it
//! receives `DELETE /acp` for that connection id. Nothing else ends it:
//! the SSE GET stream closing does not, and the transport has no idle
//! timeout. `roshera-app`'s `acp-client.ts` never sends that DELETE: a
//! page reload, a closed tab, the inactivity watchdog's rebuild and the
//! provider-epoch 404 recovery all simply open a NEW connection. Every
//! one of those left the old connection, and its node process, alive for
//! the life of the api-server. That accumulation is what this module ends.
//!
//! ## The mechanism
//!
//! A live client always holds at least one SSE GET open on its connection
//! (`acp-client.ts` opens the connection stream right after `initialize`,
//! and [`crate::goose_acp::acp_sse_keepalive`] keeps idle streams from
//! being cut by proxies). And `acp-client.ts` never re-opens an SSE stream
//! on an existing connection id: a stream that ends marks the client dead
//! and the next use builds a fresh connection. So "no SSE stream open on
//! this connection for [`grace`](AcpOrphanReaper::grace)" is a precise
//! signal that nobody will ever use the connection again, never a guess
//! about an idle-but-present user. When it holds, the reaper sends goose's
//! own `DELETE /acp` for that id through the inner `/acp` router, so
//! teardown runs through the transport's own shutdown path, the
//! `agent_activity` DELETE bookkeeping, and goose's connection teardown
//! (which unloads every session and kills its extension processes).
//!
//! Connections refused by the provider-epoch layer are covered by the
//! same signal: the client drops their streams when it reestablishes,
//! and the reaper sits INSIDE the epoch layer, so its DELETE reaches goose.
//!
//! ## In-flight turns
//!
//! A connection whose prompt turn is still in flight is not reaped until
//! that turn ends, for the reason `acp_provider_epoch.rs` gives: the
//! turn's tool calls are already landing on the timeline, and killing it
//! midway risks a half-applied geometry sequence. With no SSE stream
//! left, the turn's response can never be observed, so the deferral is
//! bounded by [`MAX_TURN_DEFERRAL`]: past it the connection is reaped
//! regardless, because an unobservable turn would otherwise pin its
//! process forever.

use axum::body::{Body, Bytes};
use axum::extract::{Request, State};
use axum::http::{header, Method};
use axum::middleware::Next;
use axum::response::Response;
use axum::Router;
use dashmap::DashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::Instant;
use tower::ServiceExt;

use crate::acp_provider_epoch::ACP_CONNECTION_ID_HEADER;

/// How long a connection may sit with no SSE stream open before it is
/// reaped. `acp-client.ts` opens its connection stream immediately after
/// `initialize` returns, so a minute is far longer than any legitimate gap,
/// and short enough that a burst of reloads cannot pile up processes.
pub(crate) const DEFAULT_ACP_ORPHAN_GRACE: Duration = Duration::from_secs(60);

/// Env override for [`DEFAULT_ACP_ORPHAN_GRACE`], in whole seconds.
pub(crate) const ACP_ORPHAN_GRACE_ENV: &str = "ROSHERA_ACP_ORPHAN_GRACE_SECS";

/// Upper bound on how long an in-flight turn may defer the reap of a
/// connection that has no SSE stream left.
pub(crate) const MAX_TURN_DEFERRAL: Duration = Duration::from_secs(30 * 60);

/// Answers "is a prompt turn in flight on this connection?".
pub(crate) type TurnInFlight = Arc<dyn Fn(&str) -> bool + Send + Sync>;

#[derive(Debug)]
struct ConnectionStreams {
    open: usize,
    /// When `open` last dropped to zero (or the connection was minted).
    orphaned_at: Option<Instant>,
}

/// Tracks open SSE streams per `/acp` connection and DELETEs connections
/// that have none left. See the module docs.
pub(crate) struct AcpOrphanReaper {
    grace: Duration,
    max_turn_deferral: Duration,
    connections: DashMap<String, ConnectionStreams>,
    /// The `/acp` router the reaper's DELETE is sent through: goose's
    /// transport plus Roshera's inner layers, WITHOUT the reaper itself
    /// or the epoch layer (a stale connection must still be deletable).
    inner: Router,
    turn_in_flight: TurnInFlight,
}

impl AcpOrphanReaper {
    pub(crate) fn new(
        grace: Duration,
        max_turn_deferral: Duration,
        inner: Router,
        turn_in_flight: TurnInFlight,
    ) -> Self {
        Self {
            grace,
            max_turn_deferral,
            connections: DashMap::new(),
            inner,
            turn_in_flight,
        }
    }

    /// Grace from [`ACP_ORPHAN_GRACE_ENV`], else [`DEFAULT_ACP_ORPHAN_GRACE`].
    /// An unparsable or zero value falls back to the default (and says so)
    /// rather than reaping live connections instantly.
    pub(crate) fn grace_from_env() -> Duration {
        match std::env::var(ACP_ORPHAN_GRACE_ENV) {
            Ok(raw) => match raw.trim().parse::<u64>() {
                Ok(secs) if secs > 0 => Duration::from_secs(secs),
                _ => {
                    tracing::warn!(
                        target: "goose_acp",
                        value = %raw,
                        "{ACP_ORPHAN_GRACE_ENV} must be a positive number of \
                         seconds; using the default orphan grace"
                    );
                    DEFAULT_ACP_ORPHAN_GRACE
                }
            },
            Err(_) => DEFAULT_ACP_ORPHAN_GRACE,
        }
    }

    pub(crate) fn grace(&self) -> Duration {
        self.grace
    }

    /// goose minted a connection (`initialize`). It starts orphaned: if no
    /// SSE stream is ever opened on it, it is reaped after the grace.
    fn connection_minted(self: &Arc<Self>, connection_id: &str) {
        self.connections.insert(
            connection_id.to_string(),
            ConnectionStreams {
                open: 0,
                orphaned_at: Some(Instant::now()),
            },
        );
        self.schedule(connection_id.to_string());
    }

    fn stream_opened(&self, connection_id: &str) {
        let mut entry =
            self.connections
                .entry(connection_id.to_string())
                .or_insert(ConnectionStreams {
                    open: 0,
                    orphaned_at: None,
                });
        entry.open += 1;
        entry.orphaned_at = None;
    }

    fn stream_closed(self: &Arc<Self>, connection_id: &str) {
        let now_orphaned = match self.connections.get_mut(connection_id) {
            Some(mut entry) => {
                entry.open = entry.open.saturating_sub(1);
                if entry.open == 0 {
                    entry.orphaned_at = Some(Instant::now());
                    true
                } else {
                    false
                }
            }
            None => false,
        };
        if now_orphaned {
            self.schedule(connection_id.to_string());
        }
    }

    fn forget(&self, connection_id: &str) {
        self.connections.remove(connection_id);
    }

    /// Called from a body `Drop`, so it must not assume a runtime: with none
    /// (process shutdown) there is nothing left to reap for anyway.
    fn schedule(self: &Arc<Self>, connection_id: String) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let reaper = Arc::clone(self);
        runtime.spawn(async move { reaper.reap_when_orphaned(connection_id).await });
    }

    /// Waits out the grace; DELETEs the connection if it is still orphaned.
    /// Returns without acting as soon as a stream re-opens or the
    /// connection is gone.
    async fn reap_when_orphaned(&self, connection_id: String) {
        let mut deferred_since: Option<Instant> = None;
        loop {
            let orphaned_at = match self.connections.get(&connection_id) {
                Some(entry) if entry.open == 0 => entry.orphaned_at,
                _ => return,
            };
            let Some(orphaned_at) = orphaned_at else {
                return;
            };
            let deadline = orphaned_at + self.grace;
            let now = Instant::now();
            if now < deadline {
                tokio::time::sleep_until(deadline).await;
                continue;
            }
            if (self.turn_in_flight)(&connection_id) {
                let since = *deferred_since.get_or_insert(now);
                if now.duration_since(since) < self.max_turn_deferral {
                    tokio::time::sleep(self.grace).await;
                    continue;
                }
                tracing::warn!(
                    target: "goose_acp",
                    connection_id = %connection_id,
                    "reaping an orphaned ACP connection whose turn never \
                     reported an end within the deferral bound"
                );
            }
            // Re-check and claim under the map entry: a stream that opened
            // while we slept wins, and only one task ever sends the DELETE.
            let claimed = self
                .connections
                .remove_if(&connection_id, |_, entry| entry.open == 0)
                .is_some();
            if !claimed {
                return;
            }
            self.send_delete(&connection_id).await;
            return;
        }
    }

    async fn send_delete(&self, connection_id: &str) {
        let request = match Request::builder()
            .method(Method::DELETE)
            .uri("/acp")
            .header(ACP_CONNECTION_ID_HEADER, connection_id)
            .body(Body::empty())
        {
            Ok(request) => request,
            Err(error) => {
                tracing::warn!(
                    target: "goose_acp",
                    connection_id = %connection_id,
                    %error,
                    "could not build the DELETE for an orphaned ACP connection"
                );
                return;
            }
        };
        match self.inner.clone().oneshot(request).await {
            Ok(response) => tracing::info!(
                target: "goose_acp",
                connection_id = %connection_id,
                status = %response.status(),
                "reaped orphaned ACP connection (no SSE stream for the grace period)"
            ),
            Err(error) => match error {},
        }
    }
}

/// Middleware recording SSE stream opens/closes and connection mints and
/// deletes for the [`AcpOrphanReaper`]. Pure observation: requests and
/// response bytes pass through untouched.
pub(crate) async fn track_acp_connections(
    State(reaper): State<Arc<AcpOrphanReaper>>,
    req: Request,
    next: Next,
) -> Response {
    let connection_id = req
        .headers()
        .get(ACP_CONNECTION_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let method = req.method().clone();
    let response = next.run(req).await;

    match (method, connection_id) {
        (Method::GET, Some(connection_id)) => {
            let is_sse = response.status().is_success()
                && response
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .is_some_and(|ct| ct.starts_with("text/event-stream"));
            if !is_sse {
                return response;
            }
            reaper.stream_opened(&connection_id);
            let (parts, body) = response.into_parts();
            let tracked = TrackedSseStream {
                inner: body.into_data_stream(),
                _guard: OpenStreamGuard {
                    reaper,
                    connection_id,
                },
            };
            Response::from_parts(parts, Body::from_stream(tracked))
        }
        (Method::DELETE, Some(connection_id)) => {
            if response.status().is_success() {
                reaper.forget(&connection_id);
            }
            response
        }
        (Method::POST, None) => {
            if let Some(minted) = response
                .headers()
                .get(ACP_CONNECTION_ID_HEADER)
                .and_then(|v| v.to_str().ok())
            {
                reaper.connection_minted(minted);
            }
            response
        }
        _ => response,
    }
}

/// Marks one SSE stream closed when the response body is dropped — which
/// is what happens when the client goes away, whether or not the stream
/// ever finished.
struct OpenStreamGuard {
    reaper: Arc<AcpOrphanReaper>,
    connection_id: String,
}

impl Drop for OpenStreamGuard {
    fn drop(&mut self) {
        self.reaper.stream_closed(&self.connection_id);
    }
}

struct TrackedSseStream {
    inner: axum::body::BodyDataStream,
    _guard: OpenStreamGuard,
}

impl futures::Stream for TrackedSseStream {
    type Item = Result<Bytes, axum::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.get_mut().inner).poll_next(cx)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use axum::http::{HeaderValue, StatusCode};
    use axum::response::IntoResponse;
    use axum::routing::any;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    const GRACE: Duration = Duration::from_millis(150);
    const SETTLE: Duration = Duration::from_millis(600);

    /// Stand-in for goose's transport: POST without a connection id mints
    /// `conn-1`; GET serves a never-ending SSE body; DELETE is recorded.
    fn stub_inner(deletes: Arc<Mutex<Vec<String>>>) -> Router {
        let handler = move |req: Request| {
            let deletes = Arc::clone(&deletes);
            async move {
                let connection_id = req
                    .headers()
                    .get(ACP_CONNECTION_ID_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                match (req.method().clone(), connection_id) {
                    (Method::POST, None) => {
                        let mut response = StatusCode::OK.into_response();
                        response
                            .headers_mut()
                            .insert(ACP_CONNECTION_ID_HEADER, HeaderValue::from_static("conn-1"));
                        response
                    }
                    (Method::GET, Some(_)) => {
                        let body = Body::from_stream(futures::stream::pending::<
                            Result<Bytes, std::convert::Infallible>,
                        >());
                        let mut response = Response::new(body);
                        response.headers_mut().insert(
                            header::CONTENT_TYPE,
                            HeaderValue::from_static("text/event-stream"),
                        );
                        response
                    }
                    (Method::DELETE, Some(id)) => {
                        deletes.lock().unwrap().push(id);
                        StatusCode::ACCEPTED.into_response()
                    }
                    _ => StatusCode::ACCEPTED.into_response(),
                }
            }
        };
        Router::new().route("/acp", any(handler))
    }

    struct Harness {
        router: Router,
        deletes: Arc<Mutex<Vec<String>>>,
        turn_active: Arc<AtomicBool>,
    }

    fn harness(max_turn_deferral: Duration) -> Harness {
        harness_with_grace(GRACE, max_turn_deferral)
    }

    fn harness_with_grace(grace: Duration, max_turn_deferral: Duration) -> Harness {
        let deletes = Arc::new(Mutex::new(Vec::new()));
        let inner = stub_inner(Arc::clone(&deletes));
        let turn_active = Arc::new(AtomicBool::new(false));
        let turn_flag = Arc::clone(&turn_active);
        let reaper = Arc::new(AcpOrphanReaper::new(
            grace,
            max_turn_deferral,
            inner.clone(),
            Arc::new(move |_| turn_flag.load(Ordering::SeqCst)),
        ));
        let router = inner.layer(axum::middleware::from_fn_with_state(
            reaper,
            track_acp_connections,
        ));
        Harness {
            router,
            deletes,
            turn_active,
        }
    }

    async fn send(router: &Router, method: Method, connection_id: Option<&str>) -> Response {
        let mut builder = Request::builder().method(method).uri("/acp");
        if let Some(id) = connection_id {
            builder = builder.header(ACP_CONNECTION_ID_HEADER, id);
        }
        router
            .clone()
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn deletes(h: &Harness) -> Vec<String> {
        h.deletes.lock().unwrap().clone()
    }

    /// The browser-reload path: the SSE stream goes away, no DELETE ever
    /// comes. The reaper must DELETE the connection itself.
    #[tokio::test]
    async fn dropped_sse_stream_without_delete_is_reaped_after_the_grace() {
        let h = harness(MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;
        let stream = send(&h.router, Method::GET, Some("conn-1")).await;
        assert_eq!(stream.status(), StatusCode::OK);
        drop(stream);

        tokio::time::sleep(SETTLE).await;
        assert_eq!(deletes(&h), vec!["conn-1".to_string()]);
    }

    /// The grace is a real wait, not a formality: an orphan is left alone
    /// until it has elapsed, and reaped once it has. Every other test here
    /// re-opens its stream (or holds it) before the reap task first runs,
    /// so a reaper that DELETEs the moment a connection is orphaned passes
    /// all of them; this is the test that fails on it.
    #[tokio::test]
    async fn orphan_is_not_reaped_before_the_grace_elapses() {
        let grace = Duration::from_millis(1000);
        let h = harness_with_grace(grace, MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;
        drop(send(&h.router, Method::GET, Some("conn-1")).await);

        tokio::time::sleep(grace / 5).await;
        assert!(
            deletes(&h).is_empty(),
            "reaped before the grace period elapsed"
        );

        tokio::time::sleep(grace + SETTLE).await;
        assert_eq!(deletes(&h), vec!["conn-1".to_string()]);
    }

    /// A live tab holds its stream open indefinitely; it is never reaped.
    #[tokio::test]
    async fn connection_with_an_open_sse_stream_is_never_reaped() {
        let h = harness(MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;
        let connection_stream = send(&h.router, Method::GET, Some("conn-1")).await;
        let session_stream = send(&h.router, Method::GET, Some("conn-1")).await;
        // One of the two streams closing leaves the connection live.
        drop(session_stream);

        tokio::time::sleep(SETTLE).await;
        assert!(deletes(&h).is_empty(), "a live connection was reaped");
        drop(connection_stream);
    }

    /// A connection minted by `initialize` that never opens a stream is
    /// abandoned from birth and is reaped too.
    #[tokio::test]
    async fn minted_connection_that_never_opens_a_stream_is_reaped() {
        let h = harness(MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;

        tokio::time::sleep(SETTLE).await;
        assert_eq!(deletes(&h), vec!["conn-1".to_string()]);
    }

    /// A stream re-opened inside the grace cancels the pending reap.
    #[tokio::test]
    async fn stream_reopened_within_the_grace_cancels_the_reap() {
        let h = harness(MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;
        drop(send(&h.router, Method::GET, Some("conn-1")).await);
        let reopened = send(&h.router, Method::GET, Some("conn-1")).await;

        tokio::time::sleep(SETTLE).await;
        assert!(deletes(&h).is_empty(), "reaped despite a re-opened stream");
        drop(reopened);
    }

    /// An in-flight turn defers the reap until it ends.
    #[tokio::test]
    async fn in_flight_turn_defers_the_reap_until_it_ends() {
        let h = harness(MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;
        h.turn_active.store(true, Ordering::SeqCst);
        drop(send(&h.router, Method::GET, Some("conn-1")).await);

        tokio::time::sleep(SETTLE).await;
        assert!(deletes(&h).is_empty(), "reaped mid-turn");

        h.turn_active.store(false, Ordering::SeqCst);
        tokio::time::sleep(SETTLE).await;
        assert_eq!(deletes(&h), vec!["conn-1".to_string()]);
    }

    /// A turn that never reports an end cannot pin the connection forever.
    #[tokio::test]
    async fn unending_turn_is_reaped_once_the_deferral_bound_passes() {
        let h = harness(Duration::from_millis(300));
        send(&h.router, Method::POST, None).await;
        h.turn_active.store(true, Ordering::SeqCst);
        drop(send(&h.router, Method::GET, Some("conn-1")).await);

        tokio::time::sleep(Duration::from_millis(1200)).await;
        assert_eq!(deletes(&h), vec!["conn-1".to_string()]);
    }

    /// A client DELETE ends tracking; the reaper never DELETEs twice.
    #[tokio::test]
    async fn client_delete_stops_tracking() {
        let h = harness(MAX_TURN_DEFERRAL);
        send(&h.router, Method::POST, None).await;
        let stream = send(&h.router, Method::GET, Some("conn-1")).await;
        send(&h.router, Method::DELETE, Some("conn-1")).await;
        drop(stream);

        tokio::time::sleep(SETTLE).await;
        assert_eq!(
            deletes(&h),
            vec!["conn-1".to_string()],
            "only the client's own DELETE"
        );
    }
}
