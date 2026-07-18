//! Auth Slice 1 — the launch gate.
//!
//! Behavioural tests that pin the security posture of the api-server's
//! HTTP surface. Every test here drives [`build_router`] end-to-end
//! through [`tower::ServiceExt::oneshot`], so it asserts what a real
//! unauthenticated caller on the wire actually receives — not what a
//! helper function returns in isolation.
//!
//! The distinction matters. Prior to this slice the codebase carried
//! `require_modify_geometry` layers, a `permission_denied` error in the
//! catalog, and an audit-tagged middleware with a documented mode
//! matrix — all of which were inert in the default configuration. Unit
//! tests of the permission helpers passed while the wire surface
//! accepted unauthenticated `DELETE`s. Only a request driven through
//! the assembled router can tell those two states apart.

#![cfg(test)]

use crate::auth_middleware::AuthPosture;
use crate::router_integration_tests::make_test_state;
use crate::{build_router, AppState};

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;

// =====================================================================
// Harness
// =====================================================================

/// Build a state whose auth posture is explicitly `Required`.
///
/// Tests state the posture they exercise rather than inheriting it from
/// the ambient process environment. Two reasons:
///
/// 1. **Determinism.** `cargo test` runs the whole binary in one
///    process; a test that mutated `ROSHERA_*` env vars would race every
///    other test in the file. The old
///    `enforce_permission_mode_matrix` test documented exactly this
///    hazard and worked around it by collapsing the matrix into a
///    single serialised test.
/// 2. **Honesty.** A test that passed only because the developer's shell
///    happened to lack a variable would be pinning the shell, not the
///    code. The default *is* separately pinned — by
///    [`posture_defaults_to_required_on_an_empty_environment`], which
///    drives the resolver with an injected getter.
///
/// Together those two assertions are airtight: "the default posture is
/// Required" and "the Required posture refuses unauthenticated calls".
async fn secure_state() -> AppState {
    let mut state = make_test_state().await;
    state.auth_posture = AuthPosture::Required;
    state
}

/// Dispatch a request through the fully-assembled router and return the
/// status plus the parsed JSON body (`Value::Null` for empty bodies).
async fn dispatch(state: &AppState, request: Request<Body>) -> (StatusCode, Value) {
    let response = build_router(state.clone())
        .oneshot(request)
        .await
        .expect("router must produce a response (oneshot is infallible)");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialise to finite bytes");
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

/// Build a credential-free request.
fn anon(method: Method, path: &str, body: Option<Value>) -> Request<Body> {
    let builder = Request::builder().method(method).uri(path);
    match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .expect("request must build"),
        None => builder.body(Body::empty()).expect("request must build"),
    }
}

// =====================================================================
// RED 1-3 — unauthenticated mutation must be refused
// =====================================================================

/// `DELETE /api/agent/parts/{id}` with no credential must be refused.
///
/// This is the single most destructive agent-facing REST call: it
/// removes a solid from the kernel model. `delete_geometry` calls
/// `enforce_permission(.., DeleteGeometry, ..)`, which — prior to this
/// slice — returned `Ok(())` unconditionally whenever
/// `ROSHERA_REQUIRE_AUTH` was unset, i.e. in the default and only
/// reachable configuration.
#[tokio::test]
async fn unauthenticated_delete_geometry_is_rejected() {
    let state = secure_state().await;
    let path = format!("/api/agent/parts/{}", Uuid::new_v4());
    let (status, _) = dispatch(&state, anon(Method::DELETE, &path, None)).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated DELETE of a part must be refused at the middleware \
         boundary before the handler ever runs — got {status}"
    );
}

/// `DELETE /api/agent/parts` with no credential must be refused.
///
/// This call clears *all* geometry. It is the worst unauthenticated
/// outcome available on the REST surface.
#[tokio::test]
async fn unauthenticated_clear_all_geometry_is_rejected() {
    let state = secure_state().await;
    let (status, _) = dispatch(&state, anon(Method::DELETE, "/api/agent/parts", None)).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated DELETE of the whole parts collection must be refused \
         — got {status}"
    );
}

/// `POST /api/geometry` with no credential must be refused.
#[tokio::test]
async fn unauthenticated_create_geometry_is_rejected() {
    let state = secure_state().await;
    let payload = json!({
        "shape_type": "box",
        "parameters": { "width": 10.0, "height": 10.0, "depth": 10.0 }
    });
    let (status, _) = dispatch(&state, anon(Method::POST, "/api/geometry", Some(payload))).await;

    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated POST that introduces geometry must be refused — got {status}"
    );
}

// =====================================================================
// RED 4 — the health surface must stay open
// =====================================================================

/// Turning auth on must not break liveness probes.
///
/// The counterpart to the tests above: a posture that refuses
/// *everything* would pass them while making the server undeployable
/// (an orchestrator that cannot health-check a container kill-loops it).
#[tokio::test]
async fn health_is_reachable_without_a_credential() {
    let state = secure_state().await;
    let (status, _) = dispatch(&state, anon(Method::GET, "/health", None)).await;

    assert!(
        status.is_success(),
        "/health must answer without a credential so orchestrators can probe \
         the container — got {status}"
    );
}

// =====================================================================
// RED 5 — login must issue a credential the middleware accepts
// =====================================================================

/// Register a user, log in, and use the returned token on a protected
/// route.
///
/// **This test fails against HEAD for a live, non-hypothetical reason.**
/// `handlers::auth::login` mints its token with `state.auth_manager`,
/// which `main()` built with a hardcoded `"secret_key"` literal. The
/// middleware verifies with `session_manager.auth_manager()`, whose key
/// comes from `load_jwt_secret`. The two never agree, so a token that
/// `login` reports as a success is rejected by the very next request.
///
/// The design spec for this slice predicted this as a *future* hazard
/// ("routing login would detonate the hardcoded secret"). It is not
/// future: `/api/auth/login` has been routed since the initial commit.
/// The bug is live today and this test is what proves it.
#[tokio::test]
async fn login_issues_a_token_the_middleware_accepts() {
    let state = secure_state().await;

    let credentials = json!({
        "username": "slice1user",
        "email": "slice1@example.test",
        "password": "Correct-Horse-9"
    });

    // Register. The route is public (you cannot present a credential
    // before you have one), so this must succeed without auth.
    let (status, body) = dispatch(
        &state,
        anon(
            Method::POST,
            "/api/auth/register",
            Some(credentials.clone()),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "registration must succeed on a clean store — got {status}: {body}"
    );

    // Log in.
    let (status, body) = dispatch(
        &state,
        anon(
            Method::POST,
            "/api/auth/login",
            Some(json!({ "username": "slice1user", "password": "Correct-Horse-9" })),
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "login must reach the handler — got {status}: {body}"
    );
    assert_eq!(
        body["success"],
        json!(true),
        "login must succeed for correct credentials — got {body}"
    );

    let token = body["token"]
        .as_str()
        .expect("a successful login must carry a token")
        .to_string();

    // The credential login just issued must be honoured by the
    // middleware. This is the assertion that fails on HEAD.
    let request = Request::builder()
        .method(Method::GET)
        .uri("/api/geometry")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request must build");
    let (status, _) = dispatch(&state, request).await;

    assert_ne!(
        status,
        StatusCode::UNAUTHORIZED,
        "the token /api/auth/login just issued must be accepted by the auth \
         middleware. A 401 here means login and the middleware are signing and \
         verifying with different keys — login would report success and every \
         subsequent request would fail."
    );
}

// =====================================================================
// RED 6 — no hardcoded signing key
// =====================================================================

/// `main.rs` must not construct an `AuthManager` with a literal key.
///
/// A source-level assertion rather than a behavioural one, because the
/// property being defended is "this literal never comes back". The repo
/// is public: a committed signing key is forgeable by anyone who reads
/// it. The behavioural consequence (login minting tokens the middleware
/// rejects) is covered by
/// [`login_issues_a_token_the_middleware_accepts`]; this test defends
/// the root cause directly so a future edit that reintroduces a second
/// manager fails here with an explicit message.
#[test]
fn main_does_not_construct_an_auth_manager_with_a_literal_key() {
    let source = include_str!("main.rs");
    assert!(
        !source.contains("AuthManager::new("),
        "main.rs must not construct an AuthManager. There is exactly one \
         AuthManager in the process — SessionManager's, keyed from \
         load_jwt_secret (env or per-process random). A second manager with a \
         different key silently splits signing from verification: login \
         succeeds and every subsequent request 401s."
    );
}

// =====================================================================
// RED 7-8 — the WebSocket command surface must require authentication
// =====================================================================
//
// This is the slice's most important gate. The WS carries the full
// geometry command surface (GeometryCommand, AICommand, TimelineCommand,
// ExportCommand); AICommand additionally spends the operator's LLM
// budget. Before this slice the `/ws` upgrade was exempt from the HTTP
// auth layer AND no message arm required authentication — the in-band
// `Authenticate` handler verified a token but used its claims only to
// build a reply, setting no connection state. A client could connect and
// send GeometryCommand without ever authenticating, in strict mode too.
//
// These tests drive a real server over a real socket, because the hole
// is precisely in the upgrade/connection path that `oneshot` cannot
// reach.

use futures::{SinkExt, StreamExt};
use std::net::SocketAddr;
use tokio_tungstenite::tungstenite::Message as WsMessage;

/// Serve `state`'s router on an ephemeral loopback port and return the
/// bound address plus the serving task's handle (dropped by the caller
/// to shut the server down).
async fn serve(state: AppState) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("must bind an ephemeral loopback port");
    let addr = listener
        .local_addr()
        .expect("bound listener has an address");
    let router = build_router(state);
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    (addr, handle)
}

/// Connect a WebSocket to `/ws`, send `command_json`, and collect every
/// text frame the server emits for a bounded window. Returns the parsed
/// frames. The bounded window is what makes the "command was executed
/// vs refused" distinction observable without hanging: a refused command
/// yields an `auth_required` error promptly; an executed command yields
/// geometry frames; either way the collection ends when the socket goes
/// idle.
async fn ws_send_and_collect(addr: SocketAddr, command_json: Value) -> Vec<Value> {
    let url = format!("ws://{addr}/ws");
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed (the /ws upgrade is public)");

    socket
        .send(WsMessage::Text(command_json.to_string().into()))
        .await
        .expect("must send the command frame");

    let frames = collect_ws_frames(&mut socket).await;
    let _ = socket.close(None).await;
    frames
}

/// Drain text frames from a socket until a *decisive* frame arrives (an
/// auth refusal, an authentication failure, or an executed-command
/// frame) or an overall deadline passes.
///
/// Breaking on a decisive frame — rather than on a fixed idle window —
/// is what makes these tests robust when the whole suite runs in
/// parallel: dozens of ephemeral servers contend for the runtime, so a
/// starved server task may be slow to emit the first frame. A short idle
/// window would then time out with an empty buffer and fail
/// spuriously. The generous per-recv timeout plus early decisive break
/// tolerate the scheduling jitter without ever waiting on a response
/// that has already arrived.
async fn collect_ws_frames<S>(socket: &mut S) -> Vec<Value>
where
    S: futures::Stream<Item = Result<WsMessage, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    let mut frames = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let recv = tokio::time::timeout(
            remaining.min(std::time::Duration::from_millis(2500)),
            socket.next(),
        )
        .await;
        match recv {
            Ok(Some(Ok(WsMessage::Text(t)))) => {
                if let Ok(v) = serde_json::from_str::<Value>(&t) {
                    let decisive = is_auth_refusal_frame(&v)
                        || is_geometry_effect_frame(&v)
                        || v.get("type").and_then(|t| t.as_str()) == Some("AuthenticationFailed");
                    frames.push(v);
                    if decisive {
                        break;
                    }
                }
            }
            Ok(Some(Ok(_))) => {}                 // ignore non-text
            Ok(Some(Err(_))) | Ok(None) => break, // socket closed
            Err(_) => break,                      // overall/idle deadline
        }
    }
    frames
}

/// A well-formed `ClientMessage::GeometryCommand` that creates a box.
///
/// The wire shape is load-bearing for the negative assertion: the frame
/// must deserialize into the real `GeometryCommand` arm so that, absent
/// the gate, it would actually execute and broadcast geometry. A frame
/// that failed to parse would trivially "not execute" and make the test
/// vacuous. `ClientMessage` is `tag="type", content="data"`;
/// `GeometryWSCommand` and its inner `ShapeParameters` follow
/// `shared-types::geometry`.
fn geometry_create_box_frame(request_id: &str) -> Value {
    json!({
        "type": "GeometryCommand",
        "data": {
            "command": {
                "cmd": "CreatePrimitive",
                "primitive_type": "Box",
                "parameters": { "params": { "width": 10.0, "height": 10.0, "depth": 10.0 } }
            },
            "request_id": request_id
        }
    })
}

/// True if a single frame is a `ServerMessage::Error` whose
/// `error_code` marks an authentication refusal. `ServerMessage` is
/// `tag="type", content="data"`, so `error_code` sits under `data`.
fn is_auth_refusal_frame(f: &Value) -> bool {
    f.get("type").and_then(|t| t.as_str()) == Some("Error")
        && f.get("data")
            .and_then(|d| d.get("error_code"))
            .and_then(|c| c.as_str())
            == Some("auth_required")
}

/// True if a single frame looks like an *executed* command — the
/// `ObjectCreated` scene broadcast (a hand-built frame with a top-level
/// `type`) or a `ServerMessage::Success`.
fn is_geometry_effect_frame(f: &Value) -> bool {
    matches!(
        f.get("type").and_then(|t| t.as_str()),
        Some("ObjectCreated") | Some("Success")
    )
}

/// True if any collected frame is an authentication refusal.
fn has_auth_refusal(frames: &[Value]) -> bool {
    frames.iter().any(is_auth_refusal_frame)
}

/// True if any collected frame indicates the command executed. Used to
/// prove the negative: an unauthenticated command must produce none.
fn has_geometry_effect(frames: &[Value]) -> bool {
    frames.iter().any(is_geometry_effect_frame)
}

/// Connect to `/ws` and send a `GeometryCommand` without ever
/// authenticating. The server must refuse it and must not execute it.
///
/// **Fails against the stage-1 tree:** the WS command surface is not yet
/// gated, so the primitive is created and an `ObjectCreated`-class frame
/// is broadcast instead of an `auth_required` error.
#[tokio::test]
async fn unauthenticated_websocket_geometry_command_is_rejected() {
    let state = secure_state().await;
    let (addr, server) = serve(state).await;

    let frames = ws_send_and_collect(addr, geometry_create_box_frame("ws-geo-1")).await;
    server.abort();

    assert!(
        has_auth_refusal(&frames),
        "an unauthenticated WebSocket GeometryCommand must be refused with an \
         `auth_required` error before it touches the kernel. Frames received: {frames:#?}"
    );
    assert!(
        !has_geometry_effect(&frames),
        "an unauthenticated WebSocket GeometryCommand must NOT execute — no \
         geometry frame may be emitted. Frames received: {frames:#?}"
    );
}

/// The same protection for `AICommand`, which additionally spends the
/// operator's LLM budget. An unauthenticated client must not be able to
/// drive the AI provider.
#[tokio::test]
async fn unauthenticated_websocket_ai_command_is_rejected() {
    let state = secure_state().await;
    let (addr, server) = serve(state).await;

    // ClientMessage is `tag="type", content="data"`; AIWSCommand is
    // internally tagged on `cmd`.
    let command = json!({
        "type": "AICommand",
        "data": {
            "command": { "cmd": "ProcessCommand", "text": "create a box", "context": null },
            "request_id": "ws-ai-1"
        }
    });

    let frames = ws_send_and_collect(addr, command).await;
    server.abort();

    assert!(
        has_auth_refusal(&frames),
        "an unauthenticated WebSocket AICommand must be refused with an \
         `auth_required` error before it reaches the LLM provider (budget \
         protection). Frames received: {frames:#?}"
    );
}

/// Positive path: a client that authenticates in-band with a valid JWT
/// may then drive the command surface. Proves the gate *opens* — a gate
/// that never opened would pass every negative test above while making
/// the WebSocket unusable.
#[tokio::test]
async fn websocket_geometry_command_after_authenticate_executes() {
    let state = secure_state().await;

    // Mint a token from the process's single AuthManager — the same one
    // the WS `Authenticate` handler verifies against, which only works
    // because the two managers were collapsed into one.
    let token = state
        .session_manager
        .auth_manager()
        .create_token("user_ws_positive", None, vec!["user".to_string()])
        .expect("token minting must succeed")
        .token;

    let (addr, server) = serve(state).await;
    let url = format!("ws://{addr}/ws");
    let (mut socket, _resp) = tokio_tungstenite::connect_async(&url)
        .await
        .expect("WebSocket upgrade must succeed");

    // Authenticate, then issue the command on the same connection.
    let authenticate = json!({
        "type": "Authenticate",
        "data": { "token": token, "request_id": "ws-auth-1" }
    });
    socket
        .send(WsMessage::Text(authenticate.to_string().into()))
        .await
        .expect("must send Authenticate");
    socket
        .send(WsMessage::Text(
            geometry_create_box_frame("ws-geo-authed")
                .to_string()
                .into(),
        ))
        .await
        .expect("must send GeometryCommand");

    let frames = collect_ws_frames(&mut socket).await;
    let _ = socket.close(None).await;
    server.abort();

    assert!(
        !has_auth_refusal(&frames),
        "an authenticated WebSocket GeometryCommand must not be auth-refused. \
         Frames received: {frames:#?}"
    );
    assert!(
        has_geometry_effect(&frames),
        "after a valid Authenticate frame, the GeometryCommand must execute and \
         emit a geometry frame. Frames received: {frames:#?}"
    );
}

/// Positive control: under the `InsecureDevBypass` posture the same
/// GeometryCommand is admitted without a credential. This proves the
/// gate keys on posture rather than refusing unconditionally — a gate
/// that refused everything would pass the two tests above while breaking
/// local development.
#[tokio::test]
async fn dev_bypass_admits_websocket_geometry_command() {
    // make_test_state defaults to InsecureDevBypass.
    let state = make_test_state().await;
    let (addr, server) = serve(state).await;

    let frames = ws_send_and_collect(addr, geometry_create_box_frame("ws-geo-dev")).await;
    server.abort();

    assert!(
        !has_auth_refusal(&frames),
        "under InsecureDevBypass a WebSocket GeometryCommand must not be \
         auth-refused. Frames received: {frames:#?}"
    );
}

// =====================================================================
// RED/RATCHET — the unprotected-route census
// =====================================================================
//
// This is the slice's most durable artifact. It is the security analogue
// of the geometry side's red-burndown ratchet: it enumerates every route
// the server mounts, partitions them by whether they are reachable
// WITHOUT a credential (the real `path_is_exempt` predicate decides —
// the test calls the production function, it does not reimplement it),
// and asserts the credential-free set equals an explicit, reviewed
// allowlist.
//
// Why this is the anti-regression lock: under `AuthPosture::Required`
// the global `auth_middleware` demands a valid credential for every
// non-exempt path, so a newly added route is credential-gated by
// default — the only way to ship an unauthenticated route is to add it
// to `path_is_exempt` (or to a `/ws/`-prefixed path). Either move
// changes the set this test computes, so it fails until a human updates
// EXPECTED_PUBLIC_ROUTES with a justification. That is exactly the
// #44-family failure this slice exists to prevent: a route silently
// becoming public without anyone deciding it should be.

/// Every route path declared in the api-server router, extracted from
/// `main.rs` source. Routes are declared as `.route("<path>", ...)`;
/// the capture tolerates the multi-line `.route(\n  "<path>",` form
/// rustfmt produces.
fn declared_route_paths() -> Vec<String> {
    let source = include_str!("main.rs");
    let re = regex::Regex::new(r#"\.route\(\s*"([^"]+)""#)
        .expect("route-extraction regex is a compile-time constant");
    re.captures_iter(source).map(|c| c[1].to_string()).collect()
}

/// The reviewed set of routes intentionally reachable without a
/// credential. Each entry is here because it must be: a health probe,
/// the WS upgrade (authenticated in-band), or a credential-issuing
/// route. Changing this set is a security decision and must be
/// deliberate — that is the whole point of pinning it.
const EXPECTED_PUBLIC_ROUTES: &[&str] = &[
    "/",                   // root / liveness
    "/health",             // readiness probe
    "/ws",                 // WebSocket upgrade — authenticated in-band
    "/ws/viewport-bridge", // debug viewport, mounted only under ROSHERA_DEV_BRIDGE
    "/api/auth/login",     // credential issue
    "/api/auth/register",  // credential issue
    "/api/auth/refresh",   // credential rotation
];

#[test]
fn unprotected_route_census_matches_reviewed_allowlist() {
    use std::collections::BTreeSet;

    let all = declared_route_paths();

    // Sanity floor: if the parse silently returned nothing (a refactor
    // of the router shape, a renamed macro), the census would vacuously
    // pass. Pin a conservative lower bound on the route count so the
    // enumeration cannot quietly become empty.
    assert!(
        all.len() >= 200,
        "route census parsed only {} routes from main.rs — expected 200+. The \
         extraction likely broke; the ratchet is not meaningfully guarding \
         anything until it is fixed.",
        all.len()
    );

    // Partition by the *production* predicate.
    let public: BTreeSet<String> = all
        .iter()
        .filter(|p| crate::auth_middleware::path_is_exempt(p))
        .cloned()
        .collect();

    let expected: BTreeSet<String> = EXPECTED_PUBLIC_ROUTES
        .iter()
        .map(|s| s.to_string())
        .collect();

    let newly_public: Vec<&String> = public.difference(&expected).collect();
    assert!(
        newly_public.is_empty(),
        "these routes are reachable WITHOUT a credential but are not in the \
         reviewed allowlist: {newly_public:?}. A route became public without \
         review. If that is intended, add it to EXPECTED_PUBLIC_ROUTES with a \
         justification; otherwise remove it from path_is_exempt."
    );

    let removed_from_surface: Vec<&String> = expected.difference(&public).collect();
    assert!(
        removed_from_surface.is_empty(),
        "these allowlisted public routes are no longer declared/exempt: \
         {removed_from_surface:?}. If a public route was removed or renamed, \
         update EXPECTED_PUBLIC_ROUTES so the ratchet keeps matching reality."
    );
}

/// Companion behavioural assertion: representative destructive routes
/// that carry no per-route permission layer are nonetheless refused
/// without a credential, because the global front door gates them. This
/// pins the property the census depends on — "non-exempt ⇒ credential
/// required" — as observable behaviour, not just an argument about
/// middleware ordering.
#[tokio::test]
async fn unlayered_destructive_routes_still_require_a_credential() {
    let state = secure_state().await;

    // These routes have NO `.route_layer(require_*)` in the router; the
    // audit called them out as unprotected. Under the default posture
    // the front door refuses them anyway.
    let cases: &[(Method, String)] = &[
        (Method::DELETE, format!("/api/sketch/{}", Uuid::new_v4())),
        (Method::DELETE, format!("/api/parts/{}", Uuid::new_v4())),
        (Method::DELETE, format!("/api/drawings/{}", Uuid::new_v4())),
        (Method::POST, "/api/viewport/clear".to_string()),
    ];

    for (method, path) in cases {
        let (status, _) = dispatch(&state, anon(method.clone(), path, None)).await;
        assert_eq!(
            status,
            StatusCode::UNAUTHORIZED,
            "unlayered destructive route {method} {path} must still be refused \
             without a credential (front-door enforcement) — got {status}"
        );
    }
}

// =====================================================================
// RED — the rate limiter must be layered
// =====================================================================

/// The built-but-unlayered rate limiter (`AuthManager::check_rate_limit`,
/// 100 requests/minute per client) must actually be on the router — and
/// must stay on under the strict/authenticated posture, where the limit
/// is meaningful (per-identity, IP fallback for the auth-exempt surface).
///
/// Fires more than the per-minute budget at a single client within the
/// window and asserts the surplus is throttled with 429. Fails if the
/// limiter is not layered (every request would return its normal
/// status). The fixture's `AuthManager` is fresh per test, so the
/// window starts empty and this cannot leak into or out of other tests.
///
/// Runs under [`AuthPosture::Required`] deliberately: this is the posture
/// in which rate-limiting is enforced. `/health` is auth-exempt so it
/// answers without a credential, but it still passes through the rate
/// limiter (layered inner to auth on the request path), keyed on the peer
/// IP — a stable client id here. The companion
/// [`rate_limiter_is_bypassed_under_the_dev_insecure_posture`] pins the
/// other half of the matrix: under the dev bypass this same flood is
/// NOT throttled.
#[tokio::test]
async fn rate_limiter_is_layered_and_throttles_a_flooding_client() {
    let state = secure_state().await;

    // Budget is 100/min. Fire 130 and confirm the surplus is throttled.
    let mut statuses = Vec::with_capacity(130);
    for _ in 0..130 {
        let (status, _) = dispatch(&state, anon(Method::GET, "/health", None)).await;
        statuses.push(status);
    }

    let throttled = statuses
        .iter()
        .filter(|s| **s == StatusCode::TOO_MANY_REQUESTS)
        .count();
    let ok = statuses.iter().filter(|s| s.is_success()).count();

    assert!(
        throttled > 0,
        "the rate limiter must throttle a client that exceeds 100 req/min with \
         429 — none of {} requests were throttled, so the limiter is not \
         layered onto the router",
        statuses.len()
    );
    // Sanity: the first batch (within budget) is admitted, so this is a
    // limiter, not a blanket rejection.
    assert!(
        ok >= 100,
        "requests within the per-minute budget must be admitted — only {ok} \
         succeeded, which looks like a blanket failure rather than throttling"
    );
}

/// Under the dev-insecure posture the per-sentinel rate limit must be
/// bypassed — a flood far past the 100/min budget is admitted in full.
///
/// This pins the fix for a live starvation bug: under
/// [`AuthPosture::InsecureDevBypass`] authentication is fully disabled and
/// every credential-free request collapses to the single permissive
/// sentinel identity (`user_id = "dev-insecure"`). A per-client limit
/// keyed on that one sentinel is a single shared bucket for the whole
/// process, so a frontend viewport that polls the backend continuously
/// alone exhausts it and drives the entire API to 429 with no recovery.
///
/// The flood targets `GET /api/parts` — a *non-exempt* route, so under
/// the dev bypass it is admitted with the sentinel `AuthInfo` injected
/// and would be keyed on that shared sentinel bucket were the limiter
/// active. With the bypass in place all 130 requests succeed.
///
/// Mutation proof: remove the `InsecureDevBypass` early-return in
/// `rate_limit_middleware` and this test fails — the surplus over 100
/// throttles on the shared sentinel bucket. Its strict-posture companion
/// [`rate_limiter_is_layered_and_throttles_a_flooding_client`] stays
/// green either way, so the two together prove the distinction is real
/// and posture-scoped, not a blanket weakening of rate-limiting.
#[tokio::test]
async fn rate_limiter_is_bypassed_under_the_dev_insecure_posture() {
    // make_test_state() resolves to AuthPosture::InsecureDevBypass.
    let state = make_test_state().await;
    assert_eq!(
        state.auth_posture,
        AuthPosture::InsecureDevBypass,
        "this test must run under the dev-insecure posture to exercise the bypass"
    );

    // Fire 130 — well past the 100/min budget — at a non-exempt route
    // that, absent the bypass, would be keyed on the shared sentinel.
    let mut statuses = Vec::with_capacity(130);
    for _ in 0..130 {
        let (status, _) = dispatch(&state, anon(Method::GET, "/api/parts", None)).await;
        statuses.push(status);
    }

    let throttled = statuses
        .iter()
        .filter(|s| **s == StatusCode::TOO_MANY_REQUESTS)
        .count();

    assert_eq!(
        throttled,
        0,
        "under the dev-insecure posture the per-sentinel rate limit must be \
         bypassed — a shared-bucket limit starves the whole API when the dev \
         frontend polls; {throttled} of {} requests were throttled",
        statuses.len()
    );
    // Every request must have been admitted (not blanket-rejected): the
    // route answers 2xx for all 130.
    let ok = statuses.iter().filter(|s| s.is_success()).count();
    assert_eq!(
        ok,
        statuses.len(),
        "all requests under the dev bypass must be admitted — only {ok} of {} \
         succeeded",
        statuses.len()
    );
}
