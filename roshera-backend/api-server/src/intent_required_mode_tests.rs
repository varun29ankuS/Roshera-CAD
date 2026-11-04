//! The intent-required mode (concern B, audit item 10 / S2, 2026-08-15
//! closeout).
//!
//! Gate 2's presence half was TS-only: `agent_intent_layer` (`main.rs`)
//! attributes a declared intent onto a recorded op when one is present, but
//! an ABSENT header takes the `_` arm and the mutating op runs anyway — a
//! REST-speaking agent could mutate the model with no declared intent at
//! all. This module pins the audit's own proposal: an opt-in mode, OFF by
//! default, switched ON by `ROSHERA_REQUIRE_INTENT=1`, so the frontend and
//! every legacy REST client are unaffected and an RL run can demand it.
//!
//! Two properties, both load-bearing:
//!   1. **Default OFF is byte-identical to today.** `IntentPosture::
//!      Optional` (`make_test_state()`'s default) must not refuse, warn, or
//!      otherwise behave differently whether the intent header is present
//!      or absent — proven on both directions, not merely "usually passes".
//!   2. **ON refuses, typed, and a declared intent lets the SAME call
//!      through.** `gate: "intent"` — the SAME gate name `roshera-mcp/src/
//!      gates.ts::intentGateRefusal` already uses — so an agent that has
//!      learned the MCP shape recognises this as the same rule reached over
//!      REST, not a new vocabulary.
//!
//! Exercises `POST /api/geometry/transform`, one of the ten gate-3 routes
//! (`refuse_unsound_base`'s own set) — chosen because its body needs only
//! `object` + `translation`, no edge/face topology to look up first.

#![cfg(test)]

use crate::durability_boot_tests::{dispatch, post};
use crate::router_integration_tests::make_test_state;
use crate::{AppState, IntentPosture};

use axum::http::StatusCode;
use serde_json::json;
use uuid::Uuid;

/// Create a plain box and return its object UUID.
async fn create_box(state: &AppState) -> Uuid {
    let (status, body) = dispatch(
        state,
        post(
            "/api/geometry/box",
            json!({ "width": 10.0, "depth": 10.0, "height": 10.0 }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create must 200; body = {body}");
    Uuid::parse_str(body["object"]["id"].as_str().expect("box uuid string"))
        .expect("box uuid must parse")
}

fn transform_request(object: Uuid, intent: Option<&str>) -> axum::http::Request<axum::body::Body> {
    let mut req = post(
        "/api/geometry/transform",
        json!({ "object": object.to_string(), "translation": [1.0, 0.0, 0.0] }),
    );
    if let Some(text) = intent {
        req.headers_mut().insert(
            "x-roshera-intent",
            text.parse().expect("header value must parse"),
        );
    }
    req
}

// =====================================================================
// 1. Default OFF — byte-identical to today, both directions
// =====================================================================

#[tokio::test]
async fn default_off_lets_a_mutation_through_with_no_intent_header() {
    let state = make_test_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(&state, transform_request(object, None)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "IntentPosture::Optional (default) must not refuse a mutation with \
         no intent header — that would be a behaviour change for every \
         existing REST client; body = {body}"
    );
}

#[tokio::test]
async fn default_off_lets_a_mutation_through_with_an_intent_header_too() {
    let state = make_test_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(
        &state,
        transform_request(object, Some("flange O120 bolt circle")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "IntentPosture::Optional must behave identically whether the header \
         is present or absent — a no-op layer, not merely a lenient one; \
         body = {body}"
    );
}

// =====================================================================
// 2. ON — refuses with no declared intent, typed
// =====================================================================

async fn required_state() -> AppState {
    let mut state = make_test_state().await;
    state.intent_posture = IntentPosture::Required;
    state
}

#[tokio::test]
async fn required_mode_refuses_a_mutation_with_no_intent_header() {
    let state = required_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(&state, transform_request(object, None)).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "IntentPosture::Required must refuse a mutation with no declared \
         intent; body = {body}"
    );
    assert_eq!(
        body["success"].as_bool(),
        Some(false),
        "refusal must carry success:false; body = {body}"
    );
    assert_eq!(
        body["error_code"].as_str(),
        Some("intent_required"),
        "must be the stable intent_required code; body = {body}"
    );
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some("intent"),
        "must be the SAME gate name gates.ts::intentGateRefusal uses; \
         body = {body}"
    );
}

/// A LITERALLY EMPTY (zero-length, after decoding) intent header is
/// treated the same as an absent one — `require_declared_intent`'s
/// `.filter(|s| !s.is_empty())`, mirroring `agent_intent_layer`'s own
/// `Some(text) if !text.is_empty()` arm. NOT whitespace-only: a header
/// that decodes to `" "` is non-empty and DOES count as declared (same as
/// `agent_intent_layer` — neither layer trims before checking emptiness).
/// This test exercises exactly the empty-string case.
#[tokio::test]
async fn required_mode_refuses_an_empty_intent_header() {
    let state = required_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(&state, transform_request(object, Some(""))).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an empty intent header must be treated as absent, not as a \
         declared (empty) intent; body = {body}"
    );
}

#[tokio::test]
async fn required_mode_lets_a_mutation_through_with_a_declared_intent() {
    let state = required_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(
        &state,
        transform_request(object, Some("flange O120 x14, bolt circle D160")),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a declared intent must let the SAME mutation proceed; body = {body}"
    );
}

/// A read-only route is never gated by this mode — it exists to constrain
/// MUTATIONS, not every REST call.
#[tokio::test]
async fn required_mode_does_not_gate_a_read_only_route() {
    let state = required_state().await;

    let (status, _body) = dispatch(&state, crate::durability_boot_tests::get("/health")).await;
    assert_ne!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "a read route must never be refused for missing intent — this mode \
         gates the ten mutating routes only"
    );
}

// =====================================================================
// 2b. AgentRequests — the production default since 2026-11-04. The rule
//     "declare intent before you build" is Roshera's (roshera-agent-
//     policy.md), not one host's: it was enforced only inside roshera-mcp,
//     so any agent reaching REST directly skipped it. Every agent client
//     identifies itself with `X-Roshera-Agent`; humans (app, viewport) do
//     not, and stay unaffected.
// =====================================================================

async fn agent_requests_state() -> AppState {
    let mut state = make_test_state().await;
    state.intent_posture = IntentPosture::AgentRequests;
    state
}

fn as_agent(
    mut req: axum::http::Request<axum::body::Body>,
) -> axum::http::Request<axum::body::Body> {
    req.headers_mut().insert(
        "x-roshera-agent",
        "some-model".parse().expect("header value must parse"),
    );
    req
}

#[tokio::test]
async fn agent_requests_refuses_an_agent_mutation_with_no_intent() {
    let state = agent_requests_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(&state, as_agent(transform_request(object, None))).await;
    assert_eq!(
        status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "an agent mutation with no declared intent must be refused; body = {body}"
    );
    assert_eq!(
        body["error_code"].as_str(),
        Some("intent_required"),
        "body = {body}"
    );
    assert_eq!(
        body["details"]["gate"].as_str(),
        Some("intent"),
        "body = {body}"
    );
}

#[tokio::test]
async fn agent_requests_lets_a_human_mutation_through_without_intent() {
    let state = agent_requests_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(&state, transform_request(object, None)).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a request with no agent header is a human (app/viewport) and must not \
         be asked for intent; body = {body}"
    );
}

#[tokio::test]
async fn agent_requests_lets_an_agent_mutation_with_intent_through() {
    let state = agent_requests_state().await;
    let object = create_box(&state).await;

    let (status, body) = dispatch(
        &state,
        as_agent(transform_request(
            object,
            Some("move the bracket 1 mm along X"),
        )),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "declared intent passes; body = {body}"
    );
}

#[test]
fn intent_posture_defaults_to_agent_requests_and_off_is_explicit() {
    assert_eq!(
        IntentPosture::from_env_with(|_| None),
        IntentPosture::AgentRequests,
        "with no setting, agent requests must be held to the intent rule"
    );
    for value in ["0", "false", "off", "OFF"] {
        assert_eq!(
            IntentPosture::from_env_with(
                |k| (k == "ROSHERA_REQUIRE_INTENT").then(|| value.to_string())
            ),
            IntentPosture::Optional,
            "ROSHERA_REQUIRE_INTENT={value} must switch the rule off explicitly"
        );
    }
}

// =====================================================================
// 3. `IntentPosture::from_env` activation itself — every test above
//    injects the posture by field mutation, which proves the ENFORCEMENT
//    but not the ACTIVATION: a typo'd env var name inside `from_env_with`
//    would ship a dead feature under a fully green suite (this repo's own
//    named failure class — a capability built, correct, and wired to
//    nothing). Mirrors `auth_middleware::auth_posture_defaults_secure_
//    and_opt_out_is_explicit` exactly.
// =====================================================================

#[test]
fn intent_posture_from_env_reads_roshera_require_intent() {
    // Empty environment → AgentRequests (the 2026-11-04 default; see 2b).
    assert_eq!(
        IntentPosture::from_env_with(|_| None),
        IntentPosture::AgentRequests,
        "a server with no ROSHERA_* variables must default to AgentRequests"
    );

    // The opt-in fires only on its own variable, set truthy.
    for value in ["1", "true", "TRUE"] {
        assert_eq!(
            IntentPosture::from_env_with(
                |k| (k == "ROSHERA_REQUIRE_INTENT").then(|| value.to_string())
            ),
            IntentPosture::Required,
            "ROSHERA_REQUIRE_INTENT={value} must select Required"
        );
    }

    // An unrecognised value is not an opt-in to Required, and not an
    // opt-OUT either — only an explicit falsey value switches the rule off
    // (asserted in `intent_posture_defaults_to_agent_requests_and_off_is_explicit`).
    for value in ["", "yes", "no"] {
        assert_eq!(
            IntentPosture::from_env_with(
                |k| (k == "ROSHERA_REQUIRE_INTENT").then(|| value.to_string())
            ),
            IntentPosture::AgentRequests,
            "ROSHERA_REQUIRE_INTENT={value:?} must leave the default in place"
        );
    }

    // An unrelated variable must not move the posture.
    assert_eq!(
        IntentPosture::from_env_with(|k| (k == "ROSHERA_DEV_INSECURE").then(|| "1".to_string())),
        IntentPosture::AgentRequests,
        "an unrelated env var must not move the posture"
    );
}
