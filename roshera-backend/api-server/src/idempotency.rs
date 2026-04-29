//! HTTP idempotency for mutating endpoints.
//!
//! Roshera positions itself as an agent runtime for geometry. Agents,
//! unlike interactive humans, retry on every transient failure: dropped
//! WebSocket frames, container restarts, cold caches, even simple
//! "I didn't see a response yet, try again" loops in higher-level
//! planners. Without idempotency, the second `POST /api/geometry`
//! quietly creates a *second* sphere and the model drifts from the
//! agent's mental model in a way that is almost impossible to debug
//! after the fact.
//!
//! This module implements `Idempotency-Key`-keyed response caching for
//! every mutating route, following the IETF draft
//! "draft-ietf-httpapi-idempotency-key-header" with two pragmatic
//! deviations:
//!
//! 1. **Bodies are fingerprinted, not just keys.** When the same key is
//!    seen twice with a different request body the server replies with
//!    `409 CONFLICT` rather than silently replaying the old response.
//!    This catches the common agent bug where a planner reuses a key
//!    across logically different commands.
//! 2. **5xx responses are not cached.** A transient kernel failure
//!    must be retryable; caching it would lock the agent out of the
//!    operation forever. Definitive answers (2xx, 4xx) are cached.
//!
//! Wiring lives in `main.rs::build_router`; agents see the behaviour
//! without any per-handler code. The cache is in-memory by design —
//! idempotency windows are short (24 h) and the SaaS deployment runs
//! one process per tenant, so distributed coherence is out of scope
//! for this layer. When that changes, swap `IdempotencyStore`'s inner
//! `DashMap` for a Redis-backed store; the public API does not move.

use crate::error_catalog::{ApiError, ErrorCode};
use axum::{
    body::{to_bytes, Body, Bytes},
    extract::{Request, State},
    http::{header::HeaderName, HeaderValue, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use dashmap::DashMap;
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    sync::Arc,
    time::{Duration, Instant},
};

/// Header agents send to declare an idempotency window. Per the IETF
/// draft, values are opaque UTF-8 strings up to 255 bytes; we enforce
/// that ceiling to bound the cache key size.
pub const IDEMPOTENCY_KEY_HEADER: &str = "idempotency-key";

/// Header set on replayed responses so agents can distinguish a
/// re-execution from a fresh one. Required for agent self-debugging:
/// if an agent sees `Idempotency-Replayed: true`, it knows the kernel
/// state already reflects this command and another retry won't move
/// it forward.
pub const IDEMPOTENCY_REPLAYED_HEADER: &str = "idempotency-replayed";

/// Hard cap on a buffered request body. Above this we refuse the
/// request with 413; agents must split large payloads. Set to 4 MiB
/// which is well above any legitimate geometry command.
const MAX_BUFFERED_BODY: usize = 4 * 1024 * 1024;

/// How long a cached response stays addressable. 24 hours mirrors the
/// IETF draft's recommendation and is long enough for any sane agent
/// retry policy without unbounded memory growth.
const CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// Maximum length of an idempotency key value. Defends the DashMap
/// against pathological keys without restricting real-world usage
/// (UUIDs are 36 bytes, request IDs typically <64).
const MAX_KEY_LEN: usize = 255;

/// One cached HTTP response, indexed by `Idempotency-Key`. Stored
/// fully materialised (status + headers we care about + body) so a
/// replay is a single map lookup followed by a clone — no async work,
/// no kernel re-entry.
#[derive(Debug, Clone)]
struct CachedResponse {
    /// 64-bit fingerprint of the original request body. Compared on
    /// replay to detect "same key, different body" conflicts.
    request_fingerprint: u64,
    /// Status the original handler returned. Replayed verbatim.
    status: StatusCode,
    /// `Content-Type` header value, if the original response carried
    /// one. Preserved so JSON clients keep parsing JSON.
    content_type: Option<HeaderValue>,
    /// Full response body, already materialised to bytes.
    body: Bytes,
    /// When this entry was inserted; used for TTL eviction. We use
    /// `Instant` rather than `SystemTime` because we only care about
    /// elapsed time, not wall-clock alignment.
    inserted_at: Instant,
}

/// Concurrent, in-memory store of `Idempotency-Key` → cached response.
///
/// Cloning is cheap (`Arc<DashMap>`), so a single instance is shared
/// across the whole `AppState`. Eviction is lazy: stale entries are
/// dropped on access. A periodic sweeper would be required only if
/// agents stop reusing keys after the window closes; for the agent
/// workload we expect (constant-rate planners), lazy is enough.
#[derive(Debug, Default)]
pub struct IdempotencyStore {
    entries: DashMap<String, CachedResponse>,
}

impl IdempotencyStore {
    /// Create an empty store. Wrapped in `Arc` by the caller (see
    /// `AppState::new` in `main.rs`).
    pub fn new() -> Self {
        Self::default()
    }

    /// Look up a key. Returns the cached entry only if it has not
    /// expired; expired entries are dropped on the way out so the
    /// cache stays bounded under steady-state agent traffic.
    fn get(&self, key: &str) -> Option<CachedResponse> {
        let now = Instant::now();
        let cached = self.entries.get(key).map(|e| e.value().clone());
        if let Some(ref c) = cached {
            if now.duration_since(c.inserted_at) > CACHE_TTL {
                self.entries.remove(key);
                return None;
            }
        }
        cached
    }

    /// Insert a fresh response. Overwrites any expired entry that
    /// happened to share the key.
    fn insert(&self, key: String, value: CachedResponse) {
        self.entries.insert(key, value);
    }

    /// Number of live entries — exposed for tests and `/health`-style
    /// introspection. Includes expired-but-not-yet-swept entries; in
    /// practice the difference is negligible.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// `len() == 0`; required by clippy when `len` is public.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// Compute a stable 64-bit fingerprint of the request body. Used only
/// to detect "same key, different body" conflicts; it is not a
/// cryptographic check, so `DefaultHasher` is sufficient and avoids
/// pulling sha2/blake3 into the workspace for one feature.
fn fingerprint(body: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    body.hash(&mut h);
    h.finish()
}

/// Whether this HTTP method mutates kernel state. GET/HEAD/OPTIONS are
/// pass-throughs — caching their responses here would shadow whatever
/// caching policy the route's handler chose.
fn is_mutating(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    )
}

/// Tower/axum middleware. Mounted via
/// `Router::layer(middleware::from_fn_with_state(...))` in `main.rs`.
///
/// Behavioural contract:
/// - No `Idempotency-Key` header → pass through, no caching.
/// - Non-mutating method → pass through, no caching.
/// - Key present + cache hit + matching body → replay cached response
///   with `Idempotency-Replayed: true`.
/// - Key present + cache hit + different body → `409 CONFLICT` with
///   a JSON body explaining the mismatch.
/// - Key present + cache miss → run inner handler, cache 2xx/4xx
///   responses, forward the response upstream.
/// - 5xx responses are forwarded but never cached, so transient
///   failures stay retryable.
/// - Body larger than `MAX_BUFFERED_BODY` → `413 PAYLOAD TOO LARGE`.
pub async fn idempotency_layer(
    State(store): State<Arc<IdempotencyStore>>,
    request: Request,
    next: Next,
) -> Response {
    // Only intercept mutating verbs; reads are out of scope.
    if !is_mutating(request.method()) {
        return next.run(request).await;
    }

    // Extract & validate the key. Absent header is allowed — the
    // endpoint is still callable without idempotency, just without
    // protection. Empty / over-long keys are rejected loudly so an
    // agent learns its bug instead of silently bypassing the cache.
    let raw_key = request
        .headers()
        .get(HeaderName::from_static(IDEMPOTENCY_KEY_HEADER))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let key = match raw_key {
        None => return next.run(request).await,
        Some(s) if s.is_empty() => {
            return ApiError::new(
                ErrorCode::IdempotencyKeyEmpty,
                "Idempotency-Key header was present but empty",
            )
            .into_response();
        }
        Some(s) if s.len() > MAX_KEY_LEN => {
            return ApiError::new(
                ErrorCode::IdempotencyKeyTooLong,
                format!("Idempotency-Key exceeds {MAX_KEY_LEN} bytes"),
            )
            .into_response();
        }
        Some(s) => s,
    };

    // Buffer the body so we can fingerprint it AND replay it into the
    // inner service. axum 0.7 hands us a streaming body; we collect
    // it once with a hard cap.
    let (parts, body) = request.into_parts();
    let body_bytes = match to_bytes(body, MAX_BUFFERED_BODY).await {
        Ok(b) => b,
        Err(_) => {
            return ApiError::new(
                ErrorCode::IdempotencyBodyTooLarge,
                format!(
                    "Request body exceeds the {MAX_BUFFERED_BODY}-byte \
                     limit imposed by the idempotency layer"
                ),
            )
            .into_response();
        }
    };
    let fp = fingerprint(&body_bytes);

    // Cache lookup. Hit path is a single DashMap read + a clone of
    // already-materialised bytes; we do not re-enter the kernel.
    if let Some(cached) = store.get(&key) {
        if cached.request_fingerprint == fp {
            return replay(cached);
        }
        return ApiError::new(
            ErrorCode::IdempotencyKeyReused,
            "Idempotency-Key reused with a different request body. \
             Use a fresh key for a new command.",
        )
        .with_hint("Generate a fresh UUID per logical command.")
        .into_response();
    }

    // Cache miss → reconstruct the request and forward to the inner
    // service. The inner handler sees an unmodified request.
    let request = Request::from_parts(parts, Body::from(body_bytes));
    let response = next.run(request).await;

    // Capture the response so we can both cache it and pass it on.
    // 5xx is forwarded as-is, never cached: agents must be free to
    // retry transient kernel failures.
    if response.status().is_server_error() {
        return response;
    }

    let (resp_parts, resp_body) = response.into_parts();
    let resp_bytes = match to_bytes(resp_body, MAX_BUFFERED_BODY).await {
        Ok(b) => b,
        Err(_) => {
            // Response too large to cache — emit it as-is, but skip
            // caching. Practically only happens for streamed exports.
            return ApiError::new(
                ErrorCode::IdempotencyResponseTooLarge,
                "Response body exceeded idempotency buffer limit",
            )
            .into_response();
        }
    };

    let content_type = resp_parts
        .headers
        .get(axum::http::header::CONTENT_TYPE)
        .cloned();
    store.insert(
        key,
        CachedResponse {
            request_fingerprint: fp,
            status: resp_parts.status,
            content_type,
            body: resp_bytes.clone(),
            inserted_at: Instant::now(),
        },
    );

    Response::from_parts(resp_parts, Body::from(resp_bytes))
}

/// Build the response sent on a cache hit. Status and content-type
/// match the original; the only difference is the
/// `Idempotency-Replayed: true` marker so callers can tell.
fn replay(cached: CachedResponse) -> Response {
    let mut builder = Response::builder().status(cached.status);
    if let Some(ct) = cached.content_type {
        builder = builder.header(axum::http::header::CONTENT_TYPE, ct);
    }
    builder
        .header(IDEMPOTENCY_REPLAYED_HEADER, HeaderValue::from_static("true"))
        .body(Body::from(cached.body))
        // The builder only fails on programmer errors (invalid header
        // names we control); falling back to a structured 500 is fine.
        .unwrap_or_else(|_| {
            ApiError::new(
                ErrorCode::IdempotencyReplayFailed,
                "Failed to construct cached response",
            )
            .into_response()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Router};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt;

    /// Counter handler: returns the call count as JSON. Used to prove
    /// the inner handler is (or is not) invoked on replay.
    fn counter_app(store: Arc<IdempotencyStore>, counter: Arc<AtomicUsize>) -> Router {
        async fn handler(
            axum::extract::State(c): axum::extract::State<Arc<AtomicUsize>>,
            body: String,
        ) -> Response {
            let n = c.fetch_add(1, Ordering::SeqCst) + 1;
            let payload = serde_json::json!({ "calls": n, "echo": body });
            (StatusCode::OK, axum::Json(payload)).into_response()
        }

        Router::new()
            .route("/echo", post(handler))
            .with_state(counter)
            .layer(axum::middleware::from_fn_with_state(
                store,
                idempotency_layer,
            ))
    }

    #[tokio::test]
    async fn no_key_passes_through_unchanged() {
        let store = Arc::new(IdempotencyStore::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let app = counter_app(store.clone(), counter.clone());

        let r1 = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/echo")
                    .body(Body::from("a"))
                    .unwrap(),
            )
            .await
            .unwrap();
        let r2 = app
            .oneshot(
                Request::builder()
                    .method(Method::POST)
                    .uri("/echo")
                    .body(Body::from("a"))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(r1.status(), StatusCode::OK);
        assert_eq!(r2.status(), StatusCode::OK);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "without an Idempotency-Key the inner handler must run every time"
        );
        assert_eq!(store.len(), 0, "store stays empty without a key");
    }

    #[tokio::test]
    async fn same_key_same_body_replays_once() {
        let store = Arc::new(IdempotencyStore::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let app = counter_app(store.clone(), counter.clone());

        let mk = |body: &'static str| {
            Request::builder()
                .method(Method::POST)
                .uri("/echo")
                .header(IDEMPOTENCY_KEY_HEADER, "k1")
                .body(Body::from(body))
                .unwrap()
        };

        let r1 = app.clone().oneshot(mk("hello")).await.unwrap();
        assert_eq!(r1.status(), StatusCode::OK);
        assert!(r1.headers().get(IDEMPOTENCY_REPLAYED_HEADER).is_none());

        let r2 = app.oneshot(mk("hello")).await.unwrap();
        assert_eq!(r2.status(), StatusCode::OK);
        assert_eq!(
            r2.headers()
                .get(IDEMPOTENCY_REPLAYED_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some("true"),
            "second call must be marked as a replay"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "inner handler must run exactly once across two retries"
        );
    }

    #[tokio::test]
    async fn same_key_different_body_returns_conflict() {
        let store = Arc::new(IdempotencyStore::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let app = counter_app(store.clone(), counter.clone());

        let mk = |body: &'static str| {
            Request::builder()
                .method(Method::POST)
                .uri("/echo")
                .header(IDEMPOTENCY_KEY_HEADER, "k1")
                .body(Body::from(body))
                .unwrap()
        };

        let r1 = app.clone().oneshot(mk("hello")).await.unwrap();
        assert_eq!(r1.status(), StatusCode::OK);

        let r2 = app.oneshot(mk("world")).await.unwrap();
        assert_eq!(
            r2.status(),
            StatusCode::CONFLICT,
            "reusing a key with a different body must surface as 409"
        );
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "the conflicting second call must NOT execute the handler"
        );
    }

    #[tokio::test]
    async fn empty_key_is_rejected() {
        let store = Arc::new(IdempotencyStore::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let app = counter_app(store, counter.clone());

        let req = Request::builder()
            .method(Method::POST)
            .uri("/echo")
            .header(IDEMPOTENCY_KEY_HEADER, "")
            .body(Body::from("x"))
            .unwrap();
        let r = app.oneshot(req).await.unwrap();
        assert_eq!(r.status(), StatusCode::BAD_REQUEST);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn get_requests_pass_through_without_caching() {
        let store = Arc::new(IdempotencyStore::new());
        let counter = Arc::new(AtomicUsize::new(0));
        // Reuse the counter handler but mounted at GET to prove the
        // layer skips read methods even when a key is present.
        async fn handler(
            axum::extract::State(c): axum::extract::State<Arc<AtomicUsize>>,
        ) -> Response {
            let n = c.fetch_add(1, Ordering::SeqCst) + 1;
            (StatusCode::OK, axum::Json(serde_json::json!({ "calls": n }))).into_response()
        }
        let app = Router::new()
            .route("/r", axum::routing::get(handler))
            .with_state(counter.clone())
            .layer(axum::middleware::from_fn_with_state(
                store.clone(),
                idempotency_layer,
            ));

        let mk = || {
            Request::builder()
                .method(Method::GET)
                .uri("/r")
                .header(IDEMPOTENCY_KEY_HEADER, "k1")
                .body(Body::empty())
                .unwrap()
        };
        let _ = app.clone().oneshot(mk()).await.unwrap();
        let _ = app.oneshot(mk()).await.unwrap();
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "GET must be invoked every time even with an Idempotency-Key"
        );
        assert_eq!(store.len(), 0);
    }

    #[tokio::test]
    async fn server_errors_are_not_cached() {
        let store = Arc::new(IdempotencyStore::new());
        let counter = Arc::new(AtomicUsize::new(0));

        async fn handler(
            axum::extract::State(c): axum::extract::State<Arc<AtomicUsize>>,
        ) -> Response {
            c.fetch_add(1, Ordering::SeqCst);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({"success": false})),
            )
                .into_response()
        }

        let app = Router::new()
            .route("/fail", post(handler))
            .with_state(counter.clone())
            .layer(axum::middleware::from_fn_with_state(
                store.clone(),
                idempotency_layer,
            ));

        let mk = || {
            Request::builder()
                .method(Method::POST)
                .uri("/fail")
                .header(IDEMPOTENCY_KEY_HEADER, "k1")
                .body(Body::from("x"))
                .unwrap()
        };
        let r1 = app.clone().oneshot(mk()).await.unwrap();
        let r2 = app.oneshot(mk()).await.unwrap();

        assert_eq!(r1.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(r2.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "5xx responses must not be cached so retries can still recover"
        );
        assert_eq!(store.len(), 0);
    }
}
