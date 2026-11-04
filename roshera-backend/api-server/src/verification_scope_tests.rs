//! Gate 6, "check what you built before you close it", enforced by the server
//! for every agent request (2026-11-04).
//!
//! The rule is Roshera's (`roshera-agent-policy.md`): an agent that changed
//! geometry under an open intent must verify the result (a full certificate
//! read, or a claim that measured geometry) before it may close that intent
//! by opening the next checkpoint — or say explicitly that it is skipping
//! verification. It was enforced only by `roshera-mcp/src/gates.ts`, so an
//! agent reaching REST directly could build and move on without ever
//! looking. State is kept per agent session (`X-Roshera-Agent-Session`), so
//! one agent's unchecked work never blocks another; humans are never asked.

#![cfg(test)]

use crate::durability_boot_tests::dispatch;
use crate::router_integration_tests::make_test_state;
use crate::AppState;

use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use serde_json::{json, Value};

fn request(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut b = Request::builder().method(method).uri(uri);
    if body.is_some() {
        b = b.header("content-type", "application/json");
    }
    b.body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .expect("request must build")
}

/// The headers every agent request from `roshera-mcp` carries.
fn as_agent(mut req: Request<Body>, session: &str) -> Request<Body> {
    let h = req.headers_mut();
    h.insert("x-roshera-agent", "test-model".parse().expect("header"));
    h.insert("x-roshera-agent-session", session.parse().expect("header"));
    h.insert(
        "x-roshera-intent",
        "mounting bracket 40 x 30 x 10".parse().expect("header"),
    );
    req
}

/// An agent builds a box; returns (solid_id, object uuid).
async fn agent_builds_box(state: &AppState, session: &str) -> (u64, String) {
    let (status, body) = dispatch(
        state,
        as_agent(
            request(
                Method::POST,
                "/api/geometry/box",
                Some(json!({ "width": 10.0, "depth": 10.0, "height": 10.0 })),
            ),
            session,
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    (
        body["solid_id"].as_u64().expect("solid_id"),
        body["object"]["id"]
            .as_str()
            .expect("object id")
            .to_string(),
    )
}

fn checkpoint(name: &str, skip_verification: bool) -> Request<Body> {
    request(
        Method::POST,
        "/api/timeline/checkpoint",
        Some(json!({ "name": name, "skip_verification": skip_verification })),
    )
}

async fn assert_refused(state: &AppState, req: Request<Body>, why: &str) {
    let (status, body) = dispatch(state, req).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{why}; body = {body}"
    );
    assert_eq!(
        body["error_code"].as_str(),
        Some("verification_required"),
        "{why}; body = {body}"
    );
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some("verification_scope"),
        "the SAME gate name roshera-mcp's gate 6 uses; body = {body}"
    );
}

async fn assert_closes(state: &AppState, req: Request<Body>, why: &str) {
    let (status, body) = dispatch(state, req).await;
    assert!(status.is_success(), "{why}; status {status}, body = {body}");
}

#[tokio::test]
async fn closing_an_intent_over_unverified_agent_work_is_refused() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    assert_refused(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "an agent that built and never looked must not close the intent",
    )
    .await;
}

#[tokio::test]
async fn a_full_certificate_read_counts_as_looking() {
    let state = make_test_state().await;
    let (solid, _) = agent_builds_box(&state, "s1").await;
    let (status, body) = dispatch(
        &state,
        as_agent(
            request(
                Method::GET,
                &format!("/api/agent/parts/{solid}/perception?full=1"),
                None,
            ),
            "s1",
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "full perception must 200; body = {body}"
    );
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "after a full certificate read the intent may close",
    )
    .await;
}

#[tokio::test]
async fn a_cheap_perception_read_does_not_count_as_looking() {
    let state = make_test_state().await;
    let (solid, _) = agent_builds_box(&state, "s1").await;
    let _ = dispatch(
        &state,
        as_agent(
            request(
                Method::GET,
                &format!("/api/agent/parts/{solid}/perception"),
                None,
            ),
            "s1",
        ),
    )
    .await;
    assert_refused(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "the ambient verdict skips the expensive checks, so it is not verification",
    )
    .await;
}

#[tokio::test]
async fn a_claim_that_measured_geometry_counts_as_looking() {
    let state = make_test_state().await;
    let (_, part) = agent_builds_box(&state, "s1").await;
    let (status, body) = dispatch(
        &state,
        as_agent(
            request(
                Method::POST,
                "/api/agent/verify-claim",
                Some(json!({
                    "expr": "v",
                    "expected": 1000.0,
                    "bindings": [{ "var": "v", "measure": { "kind": "volume", "part": part } }]
                })),
            ),
            "s1",
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "verify-claim must 200; body = {body}"
    );
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "a claim that measured the part is a look",
    )
    .await;
}

#[tokio::test]
async fn a_claim_over_constants_only_does_not_count_as_looking() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    let _ = dispatch(
        &state,
        as_agent(
            request(
                Method::POST,
                "/api/agent/verify-claim",
                Some(json!({
                    "expr": "c",
                    "expected": 2.0,
                    "bindings": [{ "var": "c", "measure": { "kind": "constant", "value": 2.0 } }]
                })),
            ),
            "s1",
        ),
    )
    .await;
    assert_refused(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "a claim that measured nothing is not a look",
    )
    .await;
}

#[tokio::test]
async fn skip_verification_closes_explicitly() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", true), "s1"),
        "the one explicit escape must close the intent",
    )
    .await;
}

#[tokio::test]
async fn one_agent_session_never_blocks_another() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s2"),
        "session s2 built nothing; s1's unchecked work is not its concern",
    )
    .await;
}

#[tokio::test]
async fn a_human_checkpoint_is_never_refused() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    assert_closes(
        &state,
        checkpoint("cover plate 40 x 30 x 3", false),
        "a person using the app is not held to the agent rule",
    )
    .await;
}

#[tokio::test]
async fn the_refusal_names_what_was_built_and_how_much() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    agent_builds_box(&state, "s1").await;
    let (_, body) = dispatch(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
    )
    .await;
    assert_eq!(
        body["details"]["unverified_count"].as_u64(),
        Some(2),
        "body = {body}"
    );
    let built = body["details"]["built"].as_array().expect("built list");
    assert!(
        built
            .iter()
            .any(|b| b.as_str() == Some("POST /api/geometry/box")),
        "the refusal must name what was built; body = {body}"
    );
}

#[tokio::test]
async fn a_failed_change_is_not_unchecked_work() {
    let state = make_test_state().await;
    let (status, body) = dispatch(
        &state,
        as_agent(
            request(
                Method::POST,
                "/api/geometry/boolean",
                Some(json!({
                    "operation": "union",
                    "object_a": uuid::Uuid::new_v4().to_string(),
                    "object_b": uuid::Uuid::new_v4().to_string()
                })),
            ),
            "s1",
        ),
    )
    .await;
    assert!(
        !status.is_success(),
        "a boolean on unknown parts must fail; body = {body}"
    );
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "nothing was built, so there is nothing to verify",
    )
    .await;
}

#[tokio::test]
async fn deleting_every_part_settles_the_tally() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    let (status, body) = dispatch(
        &state,
        as_agent(request(Method::DELETE, "/api/agent/parts", None), "s1"),
    )
    .await;
    assert!(
        status.is_success(),
        "clear parts must succeed; body = {body}"
    );
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "with every part deleted there is nothing a verification could look at",
    )
    .await;
}

#[tokio::test]
async fn a_closed_intent_starts_the_next_one_clean() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", true), "s1"),
        "explicit skip closes the first intent",
    )
    .await;
    assert_closes(
        &state,
        as_agent(checkpoint("gusset 5 mm under the cover plate", false), "s1"),
        "nothing was built under the second intent",
    )
    .await;
}

#[tokio::test]
async fn clearing_the_timeline_settles_the_tally() {
    let state = make_test_state().await;
    agent_builds_box(&state, "s1").await;
    let (status, body) = dispatch(
        &state,
        as_agent(
            request(
                Method::POST,
                "/api/timeline/clear",
                Some(json!({ "session_id": uuid::Uuid::new_v4().to_string() })),
            ),
            "s1",
        ),
    )
    .await;
    assert!(
        status.is_success(),
        "timeline clear must succeed; body = {body}"
    );
    assert_closes(
        &state,
        as_agent(checkpoint("cover plate 40 x 30 x 3", false), "s1"),
        "the work's ledger was wiped, so there is nothing left to verify",
    )
    .await;
}
