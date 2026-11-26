//! An existing sheet is put on other paper through REST (disconnection gate).
//!
//! `PATCH /api/drawings/{id}/sheet` is the production caller of the kernel's
//! `standard_drawing_on_sheet`: these tests assert the route draws the
//! registered sheet on the paper and scale asked for, keeps what was authored
//! on it, refuses -- with the overflow and the scale that would fit -- a scale
//! the paper cannot hold, and leaves the sheet as it was when it refuses.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use crate::durability_boot_tests::{dispatch, get, patch, post};
use crate::router_integration_tests::make_test_state;
use crate::AppState;
use axum::http::StatusCode;
use serde_json::{json, Value};

async fn sheet_of_a_box(state: &AppState) -> String {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 120.0, "depth": 80.0, "height": 20.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    let solid = body["solid_id"].as_u64().expect("solid_id");
    let (status, body) = dispatch(
        state,
        post(&format!("/api/parts/{solid}/drawing"), json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "make drawing; body = {body}");
    body["id"].as_str().expect("drawing id").to_string()
}

async fn drawing(state: &AppState, id: &str) -> Value {
    let (status, body) = dispatch(state, get(&format!("/api/drawings/{id}"))).await;
    assert_eq!(status, StatusCode::OK, "get drawing; body = {body}");
    body
}

fn front_scale(d: &Value) -> f64 {
    d["views"]
        .as_array()
        .expect("views")
        .iter()
        .find(|v| v["name"] == "FRONT")
        .and_then(|v| v["scale"].as_f64())
        .expect("FRONT has a scale")
}

/// Mutation: make the route install nothing (drop the write of the new sheet
/// into the registry) -> the registered sheet keeps its old paper -> RED.
#[tokio::test]
async fn the_route_puts_a_sheet_on_the_paper_and_scale_asked_for() {
    let state = make_test_state().await;
    let id = sheet_of_a_box(&state).await;
    let before = drawing(&state, &id).await;
    assert_ne!(before["sheet_size"], "A1", "fixture: not on A1 already");

    // Authored on the sheet before the change: its material.
    let (status, body) = dispatch(
        &state,
        patch(
            &format!("/api/drawings/{id}/title-block"),
            json!({ "material": "S275" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "title block; body = {body}");

    let (status, body) = dispatch(
        &state,
        patch(
            &format!("/api/drawings/{id}/sheet"),
            json!({ "sheet_size": "A1", "orientation": "landscape", "scale": 2.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "set sheet; body = {body}");
    assert_eq!(body["sheet"]["sheet_size"], "A1");
    assert_eq!(body["sheet"]["scale_label"], "2:1");
    assert_eq!(body["previous"]["sheet_size"], before["sheet_size"]);
    assert_eq!(body["certificate"]["sound"], true, "body = {body}");

    let after = drawing(&state, &id).await;
    assert_eq!(after["sheet_size"], "A1");
    assert_eq!(after["orientation"], "landscape");
    assert!((front_scale(&after) - 2.0).abs() < 1e-12);
    assert_eq!(
        after["title_block"]["material"], "S275",
        "what was authored is kept"
    );
    assert_eq!(after["id"], before["id"], "the same drawing");

    // The size alone: the scale is kept.
    let (status, body) = dispatch(
        &state,
        patch(
            &format!("/api/drawings/{id}/sheet"),
            json!({ "sheet_size": "A0" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "size only; body = {body}");
    let after = drawing(&state, &id).await;
    assert_eq!(after["sheet_size"], "A0");
    assert!(
        (front_scale(&after) - 2.0).abs() < 1e-12,
        "a size change keeps the scale"
    );
}

/// Mutation: answer a kernel refusal by drawing at the largest scale that
/// fits instead -> 200 and a shrunk sheet -> RED.
#[tokio::test]
async fn a_scale_the_paper_cannot_hold_is_refused_and_the_sheet_stands() {
    let state = make_test_state().await;
    let id = sheet_of_a_box(&state).await;
    let before = drawing(&state, &id).await;
    let (status, body) = dispatch(
        &state,
        patch(
            &format!("/api/drawings/{id}/sheet"),
            json!({ "sheet_size": "A4", "scale": 5.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "body = {body}");
    let details = &body["details"];
    assert_eq!(details["gate"], "sheet_does_not_fit", "body = {body}");
    assert!(
        details["overflow_x_mm"].as_f64().is_some_and(|v| v > 0.0),
        "the overflow is measured: {body}"
    );
    let fits = details["largest_scale_that_fits"]
        .as_f64()
        .expect("the refusal names the scale that fits");
    assert!(fits < 5.0, "{fits}");
    let after = drawing(&state, &id).await;
    assert_eq!(after["sheet_size"], before["sheet_size"], "nothing changed");
    assert!((front_scale(&after) - front_scale(&before)).abs() < 1e-12);

    // The named scale is accepted.
    let (status, body) = dispatch(
        &state,
        patch(
            &format!("/api/drawings/{id}/sheet"),
            json!({ "sheet_size": "A4", "scale": fits }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "body = {body}");
    assert_eq!(drawing(&state, &id).await["sheet_size"], "A4");

    // A drawing that does not exist is not found.
    let (status, _) = dispatch(
        &state,
        patch(
            &format!("/api/drawings/{}/sheet", uuid::Uuid::new_v4()),
            json!({ "sheet_size": "A2" }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
