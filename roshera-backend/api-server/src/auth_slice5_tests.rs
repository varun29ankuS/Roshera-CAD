//! Auth Slice 5 — API-key lifecycle over HTTP.
//!
//! Closes the #42 residual: before this slice, `AuthManager::provision_api_key`
//! had no production caller, so a running server had NO way to mint, list, or
//! revoke an API key — the only credential the MCP bridge can present
//! (`Authorization: ApiKey …`). These tests pin the whole lifecycle at the wire:
//!
//!   POST   /api/auth/keys        provision (authenticated, self-service)
//!   GET    /api/auth/keys        list own keys (no secret material)
//!   DELETE /api/auth/keys/{id}   revoke own key (durable write-through)
//!
//! Same philosophy as Slice 1: every test drives [`build_router`] end-to-end,
//! because only the assembled router can prove what a caller on the wire
//! actually receives.

#![cfg(test)]

use crate::auth_middleware::AuthPosture;
use crate::router_integration_tests::{make_test_state, make_test_state_with_database};
use crate::{build_router, AppState};

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use tower::ServiceExt;

// =====================================================================
// Harness
// =====================================================================

/// A state with auth enforced and the API-key store attached, mirroring
/// production boot (`attach_api_key_store` + rehydrate) so provisioned
/// keys are durable and revocation write-through is exercised for real.
async fn keyed_state() -> AppState {
    let mut state = make_test_state().await;
    state.auth_posture = AuthPosture::Required;
    let auth_manager = state.session_manager.auth_manager();
    auth_manager.attach_api_key_store(state.database.clone());
    auth_manager
        .load_persisted_api_keys()
        .await
        .expect("fresh in-memory store must rehydrate (to zero keys)");
    state
}

/// Dispatch through the fully-assembled router; return status + JSON body.
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

fn request(method: Method, path: &str, auth: Option<&str>, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(path);
    if let Some(header) = auth {
        builder = builder.header("Authorization", header);
    }
    match body {
        Some(v) => builder
            .header("Content-Type", "application/json")
            .body(Body::from(v.to_string()))
            .expect("request must build"),
        None => builder.body(Body::empty()).expect("request must build"),
    }
}

/// Mint a Bearer credential for `user_id` exactly as the login handler would.
fn bearer_for(state: &AppState, user_id: &str) -> String {
    let token = state
        .session_manager
        .auth_manager()
        .create_token(user_id, None, vec!["user".to_string()])
        .expect("test token must mint");
    format!("Bearer {}", token.token)
}

/// Provision a key over the wire for `user_id`; return (key_id, raw_key).
async fn provision(state: &AppState, user_id: &str, name: &str) -> (String, String) {
    let bearer = bearer_for(state, user_id);
    let (status, body) = dispatch(
        state,
        request(
            Method::POST,
            "/api/auth/keys",
            Some(&bearer),
            Some(json!({ "name": name })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "provisioning with a valid Bearer token must succeed, got body: {body}"
    );
    assert_eq!(body["success"], json!(true), "body: {body}");
    let id = body["id"].as_str().expect("key id in response").to_string();
    let raw = body["key"]
        .as_str()
        .expect("raw key must be returned exactly once at provisioning")
        .to_string();
    (id, raw)
}

// =====================================================================
// Provisioning
// =====================================================================

#[tokio::test]
async fn key_provisioning_requires_a_credential() {
    let state = keyed_state().await;
    let (status, _body) = dispatch(
        &state,
        request(
            Method::POST,
            "/api/auth/keys",
            None,
            Some(json!({ "name": "anon-attempt" })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "an unauthenticated caller must not be able to mint a credential"
    );
}

#[tokio::test]
async fn a_provisioned_key_authenticates_a_request() {
    let state = keyed_state().await;
    let (_id, raw) = provision(&state, "user_alpha", "mcp-server").await;

    assert!(
        raw.starts_with("rosh_"),
        "raw key must carry the configured display prefix, got: {raw}"
    );

    // The freshly minted key must work as a wire credential on a
    // protected route — the exact scheme the MCP bridge sends.
    let (status, body) = dispatch(
        &state,
        request(
            Method::GET,
            "/api/auth/keys",
            Some(&format!("ApiKey {raw}")),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["success"], json!(true), "body: {body}");
}

#[tokio::test]
async fn permissions_outside_the_user_baseline_are_refused() {
    let state = keyed_state().await;
    let bearer = bearer_for(&state, "user_alpha");
    let (status, body) = dispatch(
        &state,
        request(
            Method::POST,
            "/api/auth/keys",
            Some(&bearer),
            Some(json!({ "name": "escalate", "permissions": ["Admin"] })),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a permission string outside the user baseline is refused, not clamped; body: {body}"
    );
    assert_eq!(body["error"], json!("PERMISSIONS_OUTSIDE_BASELINE"));

    // The refusal must be total: no key may have been minted.
    let (status, body) = dispatch(
        &state,
        request(Method::GET, "/api/auth/keys", Some(&bearer), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        body["keys"].as_array().map(Vec::len),
        Some(0),
        "refused provisioning must not leave a key behind; body: {body}"
    );
}

// =====================================================================
// Listing
// =====================================================================

#[tokio::test]
async fn listing_returns_own_keys_without_any_secret_material() {
    let state = keyed_state().await;
    let (id, raw) = provision(&state, "user_alpha", "mcp-server").await;

    let bearer = bearer_for(&state, "user_alpha");
    let (status, body) = dispatch(
        &state,
        request(Method::GET, "/api/auth/keys", Some(&bearer), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let keys = body["keys"].as_array().expect("keys array");
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0]["id"], json!(id));
    assert_eq!(keys[0]["name"], json!("mcp-server"));
    assert_eq!(keys[0]["active"], json!(true));
    assert!(
        keys[0]["prefix"].as_str().is_some(),
        "display prefix is public metadata"
    );

    // Neither the raw secret nor its hash may appear anywhere in the body.
    let serialized = body.to_string();
    assert!(
        !serialized.contains(&raw),
        "raw key must never be listed after provisioning"
    );
    assert!(
        !serialized.contains("key_hash"),
        "the stored hash is secret material and must not be serialised"
    );
}

#[tokio::test]
async fn listing_only_shows_the_callers_own_keys() {
    let state = keyed_state().await;
    let (_a_id, _a_raw) = provision(&state, "user_alpha", "alpha-key").await;
    let (_b_id, _b_raw) = provision(&state, "user_beta", "beta-key").await;

    let bearer_b = bearer_for(&state, "user_beta");
    let (status, body) = dispatch(
        &state,
        request(Method::GET, "/api/auth/keys", Some(&bearer_b), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let keys = body["keys"].as_array().expect("keys array");
    assert_eq!(keys.len(), 1, "user_beta must not see user_alpha's keys");
    assert_eq!(keys[0]["name"], json!("beta-key"));
}

// =====================================================================
// Revocation
// =====================================================================

#[tokio::test]
async fn revoking_a_key_stops_it_authenticating() {
    let state = keyed_state().await;
    let (id, raw) = provision(&state, "user_alpha", "mcp-server").await;
    let api_key_header = format!("ApiKey {raw}");

    // Sanity: works before revocation.
    let (status, _body) = dispatch(
        &state,
        request(Method::GET, "/api/auth/keys", Some(&api_key_header), None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let bearer = bearer_for(&state, "user_alpha");
    let (status, body) = dispatch(
        &state,
        request(
            Method::DELETE,
            &format!("/api/auth/keys/{id}"),
            Some(&bearer),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["success"], json!(true), "body: {body}");

    let (status, _body) = dispatch(
        &state,
        request(Method::GET, "/api/auth/keys", Some(&api_key_header), None),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "a revoked key must be refused on the very next request"
    );
}

#[tokio::test]
async fn a_key_cannot_be_revoked_by_another_user() {
    let state = keyed_state().await;
    let (a_id, a_raw) = provision(&state, "user_alpha", "alpha-key").await;

    let bearer_b = bearer_for(&state, "user_beta");
    let (status, body) = dispatch(
        &state,
        request(
            Method::DELETE,
            &format!("/api/auth/keys/{a_id}"),
            Some(&bearer_b),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "another user's key id must read as not-found (no existence leak); body: {body}"
    );

    // Alpha's key must still authenticate.
    let (status, _body) = dispatch(
        &state,
        request(
            Method::GET,
            "/api/auth/keys",
            Some(&format!("ApiKey {a_raw}")),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "the key must be untouched");
}

#[tokio::test]
async fn revoking_an_unknown_key_is_a_typed_not_found() {
    let state = keyed_state().await;
    let bearer = bearer_for(&state, "user_alpha");
    let (status, body) = dispatch(
        &state,
        request(
            Method::DELETE,
            "/api/auth/keys/no-such-key-id",
            Some(&bearer),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["error"], json!("KEY_NOT_FOUND"));
}

// =====================================================================
// Durability
// =====================================================================

#[tokio::test]
async fn a_revoked_key_stays_revoked_across_a_restart() {
    let state = keyed_state().await;
    let (id, revoked_raw) = provision(&state, "user_alpha", "to-revoke").await;
    let (_live_id, live_raw) = provision(&state, "user_alpha", "to-keep").await;

    let bearer = bearer_for(&state, "user_alpha");
    let (status, _body) = dispatch(
        &state,
        request(
            Method::DELETE,
            &format!("/api/auth/keys/{id}"),
            Some(&bearer),
            None,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    // Simulated restart: a fresh process state over the SAME database,
    // rehydrated exactly as production boot does (Slice 3 path).
    let mut restarted = make_test_state_with_database(state.database.clone(), None).await;
    restarted.auth_posture = AuthPosture::Required;
    let auth_manager = restarted.session_manager.auth_manager();
    auth_manager.attach_api_key_store(restarted.database.clone());
    let restored = auth_manager
        .load_persisted_api_keys()
        .await
        .expect("persisted keys must rehydrate after restart");
    assert!(
        restored >= 2,
        "both keys must have been persisted (restored {restored})"
    );

    let (status, _body) = dispatch(
        &restarted,
        request(
            Method::GET,
            "/api/auth/keys",
            Some(&format!("ApiKey {live_raw}")),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "the live key must survive a restart"
    );

    let (status, _body) = dispatch(
        &restarted,
        request(
            Method::GET,
            "/api/auth/keys",
            Some(&format!("ApiKey {revoked_raw}")),
            None,
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "revocation must survive a restart — a revoked key that resurrects on reboot is the Slice-3 defect all over again"
    );
}
