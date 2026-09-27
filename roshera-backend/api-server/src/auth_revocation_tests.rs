//! Task 72 — logout and refresh over the wire, when the revocation store
//! cannot record them, and the boot path that restores what it did record.
//!
//! The session-manager suite (`tests/token_revocation_persistence.rs`)
//! proves the `AuthManager` contract across a restart with a stable secret.
//! These tests pin what a CALLER of `/api/auth/logout` and
//! `/api/auth/refresh` is told, and that production boot
//! ([`crate::restore_durable_auth`]) loads the revocations a logout wrote.
//!
//! A real write failure without a fake store: the database file is migrated
//! read-write, then reopened READ-ONLY for the server under test, so every
//! insert into `revoked_tokens` fails inside SQLite itself.

#![cfg(test)]

use crate::auth_middleware::AuthPosture;
use crate::durability_boot_tests::{open_db, temp_db_path, Db};
use crate::handlers::auth::{LOGOUT_NOT_DURABLE_MESSAGE, REFRESH_NOT_DURABLE_MESSAGE};
use crate::router_integration_tests::make_test_state_with_database;
use crate::{build_router, restore_durable_auth, AppState};

use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};
use session_manager::{DatabaseConfig, DatabaseType, SessionToken, SqliteDatabase};
use std::sync::Arc;
use tower::ServiceExt;

/// A migrated database file reopened read-only: reads work, every write
/// fails.
async fn read_only_db() -> Db {
    let path = temp_db_path();
    drop(open_db(&path).await);
    let cfg = DatabaseConfig {
        db_type: DatabaseType::SQLite,
        url: format!("sqlite://{path}?mode=ro"),
        max_connections: 2,
        connect_timeout: 5,
        run_migrations: false,
    };
    Arc::new(
        SqliteDatabase::new(&cfg)
            .await
            .expect("a migrated sqlite file must open read-only"),
    )
}

/// A state with auth enforced, booted through the production auth restore.
async fn booted_state(db: Db) -> AppState {
    let mut state = make_test_state_with_database(db, None, None).await;
    state.auth_posture = AuthPosture::Required;
    restore_durable_auth(&state)
        .await
        .expect("the production auth restore must succeed over a readable store");
    state
}

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

fn mint(state: &AppState, user_id: &str) -> SessionToken {
    state
        .session_manager
        .auth_manager()
        .create_token(
            user_id,
            None,
            vec!["user".to_string()],
            session_manager::PrincipalKind::Human,
        )
        .expect("test token must mint")
}

/// Over the read-only store, a refresh token that is NOT revoked is stopped
/// only by the store (`PersistenceError`, claim withdrawn, nothing spent); a
/// revoked one would be refused `AccessDenied` from memory first.
async fn assert_unrevoked(state: &AppState, refresh: &str) {
    match state
        .session_manager
        .auth_manager()
        .rotate_refresh_token(refresh)
        .await
    {
        Err(session_manager::SessionError::PersistenceError { .. }) => {}
        other => panic!(
            "the refresh token must still be unrevoked (only the store may stop it), got {:?}",
            other.map(|t| t.id)
        ),
    }
}

fn logout_request(token: &SessionToken) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/api/auth/logout")
        .header("Authorization", format!("Bearer {}", token.token))
        .body(Body::empty())
        .expect("request must build")
}

#[tokio::test]
async fn a_logout_the_store_cannot_record_is_refused_and_the_session_stays_live() {
    let state = booted_state(read_only_db().await).await;
    let token = mint(&state, "user_logout_ro");

    let (status, body) = dispatch(&state, logout_request(&token)).await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a logout the store refused must not answer 200, got body: {body}"
    );
    assert_eq!(body["success"], json!(false), "body: {body}");
    assert_eq!(body["error"], json!("LOGOUT_NOT_RECORDED"), "body: {body}");
    let message = body["message"].as_str().unwrap_or_default();
    assert_eq!(message, LOGOUT_NOT_DURABLE_MESSAGE);
    assert!(!message.contains("  "), "message: {message:?}");

    // The caller was told the session is still active — it must be.
    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::GET)
            .uri("/api/auth/keys")
            .header("Authorization", format!("Bearer {}", token.token))
            .body(Body::empty())
            .expect("request must build"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "after a refused logout the bearer must still authenticate, got body: {body}"
    );
    let refresh = token.refresh_token.clone().expect("refresh token minted");
    assert_unrevoked(&state, &refresh).await;
}

#[tokio::test]
async fn a_refresh_the_store_cannot_record_is_refused_without_calling_the_token_invalid() {
    let state = booted_state(read_only_db().await).await;
    let refresh = mint(&state, "user_refresh_ro")
        .refresh_token
        .expect("refresh token minted");

    let (status, body) = dispatch(
        &state,
        Request::builder()
            .method(Method::POST)
            .uri("/api/auth/refresh")
            .header("Content-Type", "application/json")
            .body(Body::from(json!({ "refresh_token": refresh }).to_string()))
            .expect("request must build"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::SERVICE_UNAVAILABLE,
        "a refresh the store refused must not be reported as an invalid token, got body: {body}"
    );
    assert_eq!(body["success"], json!(false), "body: {body}");
    let error = body["error"].as_str().unwrap_or_default();
    assert_eq!(error, REFRESH_NOT_DURABLE_MESSAGE);
    assert!(!error.contains("  "), "error: {error:?}");

    assert_unrevoked(&state, &refresh).await;
}

#[tokio::test]
async fn boot_restores_the_revocations_a_logout_recorded() {
    let path = temp_db_path();
    let db = open_db(&path).await;

    let before = booted_state(db.clone()).await;
    let token = mint(&before, "user_logout_rw");
    let (status, body) = dispatch(&before, logout_request(&token)).await;
    assert_eq!(status, StatusCode::OK, "body: {body}");
    assert_eq!(body["success"], json!(true), "body: {body}");

    // Simulated restart: a fresh process state over the SAME database,
    // booted through the production restore.
    let after = make_test_state_with_database(db, None, None).await;
    let restored = restore_durable_auth(&after)
        .await
        .expect("the production auth restore must succeed");
    assert_eq!(
        restored.revocations, 2,
        "the logout's access and refresh revocations must both come back at boot"
    );
}
