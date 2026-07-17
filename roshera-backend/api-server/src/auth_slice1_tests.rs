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
