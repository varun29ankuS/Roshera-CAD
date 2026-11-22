//! Sheet orientation reaches the kernel from REST (disconnection gate).
//!
//! The kernel lays a sheet landscape or portrait and its automatic search
//! tries both; these tests assert the production routes carry an engineer's
//! or agent's choice through to the sheet they register: the one-call
//! creation route (`POST /api/parts/{id}/drawing?orientation=`, the route the
//! MCP `make_drawing` tool calls) and the blank-sheet route
//! (`POST /api/drawings` with `orientation`). The recorded creation event
//! carries the orientation asked for, which is what a rebuild from history
//! reads (`timeline_engine::replay::rederive_part_drawing`).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::durability_boot_tests::{dispatch, get, post};
use crate::router_integration_tests::make_test_state;
use crate::AppState;
use axum::http::StatusCode;
use serde_json::{json, Value};

async fn create_box(state: &AppState) -> u64 {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 40.0, "depth": 30.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    body["solid_id"].as_u64().expect("solid_id")
}

async fn drawing(state: &AppState, id: &str) -> Value {
    let (status, body) = dispatch(state, get(&format!("/api/drawings/{id}"))).await;
    assert_eq!(status, StatusCode::OK, "get drawing; body = {body}");
    body
}

/// `?orientation=portrait` draws the registered sheet on portrait paper and
/// `?orientation=landscape` on landscape paper; without it the search
/// chooses (a flat box: landscape).
///
/// Mutation: drop `q.orientation` from the route's call into the kernel
/// (pass `None`) -> the portrait request comes back landscape -> RED.
#[tokio::test]
async fn the_creation_route_draws_on_the_orientation_asked_for() {
    let state = make_test_state().await;
    let solid = create_box(&state).await;
    for (query, want) in [
        ("?orientation=portrait", "portrait"),
        ("?orientation=landscape", "landscape"),
        ("", "landscape"),
    ] {
        let (status, body) = dispatch(
            &state,
            post(&format!("/api/parts/{solid}/drawing{query}"), json!({})),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{query}: body = {body}");
        let id = body["id"].as_str().expect("drawing id").to_string();
        let d = drawing(&state, &id).await;
        assert_eq!(d["orientation"], want, "{query}: {}", d["sheet_size"]);
    }
    // A value that is not an orientation is refused, not ignored.
    let (status, _) = dispatch(
        &state,
        post(
            &format!("/api/parts/{solid}/drawing?orientation=sideways"),
            json!({}),
        ),
    )
    .await;
    assert!(status.is_client_error(), "sideways: {status}");
}

/// The blank-sheet route takes an orientation, and reads its absence as
/// landscape (every sheet created before the choice existed).
#[tokio::test]
async fn a_blank_sheet_is_created_on_the_orientation_asked_for() {
    let state = make_test_state().await;
    let (status, body) = dispatch(
        &state,
        post(
            "/api/drawings",
            json!({ "name": "tall", "sheet_size": "A4", "orientation": "portrait" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let d = drawing(&state, body["id"].as_str().expect("id")).await;
    assert_eq!(d["orientation"], "portrait");
    assert_eq!(d["sheet_size"], "A4");

    let (status, body) = dispatch(
        &state,
        post(
            "/api/drawings",
            json!({ "name": "plain", "sheet_size": "A3" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let d = drawing(&state, body["id"].as_str().expect("id")).await;
    assert_eq!(d["orientation"], "landscape");

    // A custom sheet's dimensions say which way up it is: an orientation
    // that contradicts them is refused, never silently overridden; one that
    // agrees is taken.
    let tall = json!({ "CUSTOM": { "width": 300.0, "height": 500.0 } });
    let (status, body) = dispatch(
        &state,
        post(
            "/api/drawings",
            json!({ "name": "c", "sheet_size": tall, "orientation": "landscape" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body = {body}");
    let (status, body) = dispatch(
        &state,
        post(
            "/api/drawings",
            json!({ "name": "c", "sheet_size": tall, "orientation": "portrait" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    let d = drawing(&state, body["id"].as_str().expect("id")).await;
    assert_eq!(d["orientation"], "portrait");
}
