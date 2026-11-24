//! Router-integration tests for the kernel-served tool registry
//! (`GET /api/agent/tool-registry`) — Slice 1 / Layer 0 of the MCP scale
//! architecture (spec 2026-07-20 §2 Layer 0, §3 honesty contract).
//!
//! RED-first: authored before `agent_registry.rs` exists, so every test
//! here fails (404 / missing shape) until the endpoint + module land. The
//! tests drive the LIVE router through `build_router` + `oneshot`, exactly
//! like `router_integration_tests`, so they exercise URL routing, the
//! middleware stack, and the full response pipeline.
//!
//! Three gates:
//!  (a) SHAPE — 200, ≥85 tools, every entry has non-empty name/bench/purpose
//!      /schema, all benches ∈ the allowed set, `registry_hash` stable across
//!      two calls.
//!  (b) COMPLETENESS PIN — the registry contains an entry for EVERY tool name
//!      the MCP server currently exposes. The current MCP surface is encoded
//!      below as an explicit fixture (extracted from
//!      `roshera-mcp/src/tools/*.ts` `server.tool("name"…)` /
//!      `server.registerTool("name"…)` registrations). A future MCP tool added
//!      without a matching registry entry fails this test.
//!  (c) SCHEMA VALIDITY — every `input_schema` parses as a JSON object whose
//!      `type` is `"object"`.

#![cfg(test)]

use crate::router_integration_tests::make_test_state;
use crate::{build_router, AppState};
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request, StatusCode};
use serde_json::Value;
use tower::ServiceExt;

/// The six benches + core (spec §Layer 1; open-Q1 resolution: GD&T lives
/// inside the drawing bench; `timeline` added 2026-07-31 for history/
/// branching/certification). The registry MUST classify every tool into
/// exactly one of these.
const ALLOWED_BENCHES: &[&str] = &[
    "core", "sketch", "assembly", "drawing", "analysis", "simulate", "labels", "timeline",
];

/// The tool names `roshera-mcp` classifies today, read from ITS OWN table.
///
/// This used to be `MCP_TOOL_NAMES`, a hand-kept list in this file, justified
/// as a completeness pin: "a new MCP tool must be added here AND given a
/// registry entry in the same change — otherwise this test goes red."
///
/// The mechanism is backwards, and it failed silently for six tools. The list
/// only goes red when somebody remembers to update it, which is precisely the
/// step that gets forgotten; `ask_choice`, `cad_program`, `document_rename`,
/// `psketch_plane_from_face`, `recipe_get` and `workbench` all reached the MCP
/// AND the backend registry while this constant sat at 102 and the gate
/// reported green. A hand-kept copy of a surface is a third surface, and the
/// whole point of a drift gate is to compare two INDEPENDENTLY MAINTAINED
/// ones. So it now reads `BENCH_OF` — the classification the MCP actually
/// ships — and a tool added there is visible here the moment it exists.
///
/// Parse failure is a HARD failure, never an empty set: a gate that checked
/// nothing must not be indistinguishable from a gate that passed.
fn mcp_tool_names() -> Vec<String> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("roshera-mcp")
        .join("src")
        .join("registry.ts");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read roshera-mcp's registry at {}: {e}",
            path.display()
        )
    });

    let after = source
        .split_once("BENCH_OF")
        .unwrap_or_else(|| panic!("registry.ts no longer contains BENCH_OF"))
        .1;
    let body = after
        .split_once("};")
        .unwrap_or_else(|| panic!("BENCH_OF is not terminated by a closing brace"))
        .0;

    let mut names: Vec<String> = Vec::new();
    for line in body.lines() {
        let line = line.trim();
        if line.starts_with("//") {
            continue;
        }
        let Some((key, _)) = line.split_once(':') else {
            continue;
        };
        let key = key.trim().trim_matches('"').trim_matches('\'');
        if !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            names.push(key.to_string());
        }
    }

    // A floor, not a count: pinning the exact number would recreate the
    // hand-maintained constant this replaced. This only catches the case where
    // the parse broke and returned a set too small to be real.
    assert!(
        names.len() > 80,
        "parsed only {} tool names from roshera-mcp's BENCH_OF — the parse is \
         broken, and an empty-ish set would make this gate pass without \
         checking anything",
        names.len()
    );
    names
}

/// GET a URI through the live router and return `(status, json_body)`.
async fn get(state: &AppState, uri: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::GET)
        .uri(uri)
        .body(Body::empty())
        .expect("static request must build");
    let response = build_router(state.clone())
        .oneshot(request)
        .await
        .expect("router must produce a response (oneshot infallibility)");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialize to finite bytes");
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, body)
}

/// (a) SHAPE + hash stability.
#[tokio::test]
async fn tool_registry_shape_and_hash_stable() {
    let state = make_test_state().await;

    let (status, body) = get(&state, "/api/agent/tool-registry").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/agent/tool-registry must return 200; body = {body}"
    );

    let tools = body["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("`tools` must be an array; body = {body}"));
    assert!(
        tools.len() >= 85,
        "registry must expose >=85 tools, got {}",
        tools.len()
    );

    for t in tools {
        let name = t["name"].as_str().unwrap_or("");
        assert!(!name.is_empty(), "every tool needs a non-empty name: {t}");
        let bench = t["bench"].as_str().unwrap_or("");
        assert!(
            ALLOWED_BENCHES.contains(&bench),
            "tool `{name}` has bench `{bench}` not in the allowed set {ALLOWED_BENCHES:?}"
        );
        let purpose = t["purpose"].as_str().unwrap_or("");
        assert!(
            !purpose.is_empty(),
            "tool `{name}` needs a non-empty purpose"
        );
        assert!(
            t["input_schema"].is_object(),
            "tool `{name}` needs an object input_schema"
        );
        assert!(
            t["token_estimate"].as_u64().is_some(),
            "tool `{name}` needs a numeric token_estimate"
        );
        let stability = t["stability"].as_str().unwrap_or("");
        assert!(
            stability == "stable" || stability == "experimental",
            "tool `{name}` has stability `{stability}` (must be stable|experimental)"
        );
    }

    let hash1 = body["registry_hash"]
        .as_str()
        .unwrap_or_else(|| panic!("`registry_hash` must be a string; body = {body}"));
    assert!(!hash1.is_empty(), "registry_hash must be non-empty");
    assert!(
        body["generated_at"].as_str().is_some(),
        "`generated_at` must be present"
    );

    // Hash is a pure function of the tools content — identical across calls.
    let (_s2, body2) = get(&state, "/api/agent/tool-registry").await;
    let hash2 = body2["registry_hash"].as_str().unwrap_or("");
    assert_eq!(
        hash1, hash2,
        "registry_hash must be stable across two calls (drift detection contract)"
    );
}

/// (b) COMPLETENESS PIN — every current MCP tool has a registry entry.
#[tokio::test]
async fn tool_registry_covers_every_mcp_tool() {
    let state = make_test_state().await;
    let (status, body) = get(&state, "/api/agent/tool-registry").await;
    assert_eq!(status, StatusCode::OK, "endpoint must 200; body = {body}");

    let tools = body["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("`tools` must be an array; body = {body}"));
    let names: std::collections::HashSet<&str> =
        tools.iter().filter_map(|t| t["name"].as_str()).collect();

    let mcp = mcp_tool_names();
    let missing: Vec<&String> = mcp.iter().filter(|n| !names.contains(n.as_str())).collect();
    assert!(
        missing.is_empty(),
        "registry is missing entries for {} of the {} tools roshera-mcp \
         classifies today: {missing:?}",
        missing.len(),
        mcp.len()
    );
}

/// (c) SCHEMA VALIDITY — every input_schema is a JSON object of type "object".
#[tokio::test]
async fn tool_registry_schemas_are_object_typed() {
    let state = make_test_state().await;
    let (status, body) = get(&state, "/api/agent/tool-registry").await;
    assert_eq!(status, StatusCode::OK, "endpoint must 200; body = {body}");

    for t in body["tools"].as_array().expect("tools array") {
        let name = t["name"].as_str().unwrap_or("<unnamed>");
        let schema = &t["input_schema"];
        assert!(schema.is_object(), "tool `{name}` schema must be an object");
        assert_eq!(
            schema["type"].as_str(),
            Some("object"),
            "tool `{name}` input_schema.type must be \"object\""
        );
    }
}

/// (e) The backend-owned definitions are SERVED: `GET /api/agent/tools` is the
/// route roshera-mcp lists them from, and `/api/agent/tool-registry` names the
/// same tools, so the two listings cannot disagree.
#[tokio::test]
async fn agent_tools_route_serves_the_owned_definitions() {
    let state = make_test_state().await;
    let (status, body) = get(&state, "/api/agent/tools").await;
    assert_eq!(
        status,
        StatusCode::OK,
        "GET /api/agent/tools; body = {body}"
    );
    assert_eq!(body["registry_version"].as_str(), Some("2.0.0"), "{body}");
    let tools = body["tools"]
        .as_array()
        .unwrap_or_else(|| panic!("`tools` must be an array; body = {body}"));
    assert_eq!(tools.len(), 12, "the assembly bench; body = {body}");
    for t in tools {
        let name = t["name"].as_str().unwrap_or("");
        assert!(name.starts_with("assembly_"), "{t}");
        assert!(t["call"].is_object(), "{name} must say how it is called");
        assert!(
            t["examples"].as_array().is_some_and(|a| !a.is_empty()),
            "{name} must carry an example"
        );
        assert_eq!(t["input_schema"]["additionalProperties"], false, "{name}");
    }

    let (_, registry) = get(&state, "/api/agent/tool-registry").await;
    let listed: std::collections::HashSet<&str> = registry["tools"]
        .as_array()
        .map(|a| a.iter().filter_map(|t| t["name"].as_str()).collect())
        .unwrap_or_default();
    for t in tools {
        let name = t["name"].as_str().unwrap_or("");
        assert!(
            listed.contains(name),
            "{name} served but not in the tool registry"
        );
    }
}

/// POST a JSON body through the live router and return `(status, json_body)`.
async fn post(state: &AppState, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("static request must build");
    let response = build_router(state.clone())
        .oneshot(request)
        .await
        .expect("router must produce a response (oneshot infallibility)");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("response body must serialize to finite bytes");
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// (f) `assembly_mate`'s route is live and refuses typed: an unknown assembly
/// is a catalogue error, and a pid that is not the canonical 32-hex form is a
/// typed `invalid_parameter` naming the form — not axum's bare-text 422.
#[tokio::test]
async fn joint_route_is_routed_and_refuses_typed() {
    let state = make_test_state().await;
    let end = |pid: &str| {
        serde_json::json!({
            "instance_id": "22222222-2222-4222-8222-222222222222",
            "pid": pid,
        })
    };
    let uri = "/api/assembly/11111111-1111-4111-8111-111111111111/joint";

    let (status, body) = post(
        &state,
        uri,
        serde_json::json!({
            "kind": "Fastened",
            "a": end("0123456789abcdef0123456789abcdef"),
            "b": end("fedcba9876543210fedcba9876543210"),
        }),
    )
    .await;
    assert_ne!(
        status,
        StatusCode::METHOD_NOT_ALLOWED,
        "route must exist; {body}"
    );
    assert!(
        body["error_code"].is_string(),
        "typed refusal for an unknown assembly: {body}"
    );

    let (status, body) = post(
        &state,
        uri,
        serde_json::json!({ "kind": "Fastened", "a": end("12345"), "b": end("12345") }),
    )
    .await;
    assert!(status.is_client_error(), "{status} {body}");
    assert_eq!(
        body["error_code"].as_str(),
        Some("invalid_parameter"),
        "{body}"
    );
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("32 lowercase hex"),
        "the refusal names the canonical pid form: {body}"
    );
}

/// (g) `set_part_color` refuses a part that does not exist (typed
/// `solid_not_found`) and still colours one that does. RED before
/// 2026-11-18: the route answered 200 `success: true` for any integer and
/// recorded the colour in history.
#[tokio::test]
async fn set_part_color_refuses_a_part_that_does_not_exist() {
    let state = make_test_state().await;
    let (status, body) = post(
        &state,
        "/api/agent/parts/987654/color",
        serde_json::json!({ "r": 10, "g": 20, "b": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "{body}");
    assert_eq!(
        body["error_code"].as_str(),
        Some("solid_not_found"),
        "{body}"
    );
    assert!(
        state.solid_colors.get(&987_654).is_none(),
        "no colour is stored for a part that does not exist"
    );

    let (status, body) = post(
        &state,
        "/api/geometry/box",
        serde_json::json!({ "width": 5.0, "depth": 5.0, "height": 5.0 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "box create; body = {body}");
    let sid: u32 = {
        let m = state.model.read().await;
        let ids: Vec<u32> = m.solids.iter().map(|(id, _)| id as u32).collect();
        ids.into_iter().next().unwrap_or(u32::MAX)
    };
    let (status, body) = post(
        &state,
        &format!("/api/agent/parts/{sid}/color"),
        serde_json::json!({ "r": 10, "g": 20, "b": 30 }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["color"], serde_json::json!([10, 20, 30]), "{body}");
}

/// (h) The routes agent tools reach answer refusals with a TYPED body (the
/// error catalogue), never a bare status code with an empty body. RED before
/// 2026-11-18: every case below answered its status with no body at all, so
/// MCP's `fail()` had no `error_code` to surface (MCP_AUDIT.md §2).
#[tokio::test]
async fn tool_reached_routes_refuse_with_typed_bodies() {
    let state = make_test_state().await;
    let cases: Vec<(&str, StatusCode, Value)> = vec![
        (
            "GET /api/timeline/history/not-a-branch",
            StatusCode::BAD_REQUEST,
            get(&state, "/api/timeline/history/not-a-branch").await.1,
        ),
        (
            "GET /api/timeline/rebuild-certificate/not-a-branch",
            StatusCode::BAD_REQUEST,
            get(&state, "/api/timeline/rebuild-certificate/not-a-branch")
                .await
                .1,
        ),
        (
            "GET /api/timeline/scrub/not-a-branch/1",
            StatusCode::BAD_REQUEST,
            get(&state, "/api/timeline/scrub/not-a-branch/1").await.1,
        ),
        (
            "POST /api/timeline/undo {}",
            StatusCode::BAD_REQUEST,
            post(&state, "/api/timeline/undo", serde_json::json!({}))
                .await
                .1,
        ),
        (
            "POST /api/timeline/redo bad session",
            StatusCode::BAD_REQUEST,
            post(
                &state,
                "/api/timeline/redo",
                serde_json::json!({ "session_id": "nope" }),
            )
            .await
            .1,
        ),
        (
            "POST /api/timeline/clear bad session",
            StatusCode::BAD_REQUEST,
            post(
                &state,
                "/api/timeline/clear",
                serde_json::json!({ "session_id": "nope" }),
            )
            .await
            .1,
        ),
        (
            "GET /api/lens/987654",
            StatusCode::NOT_FOUND,
            get(&state, "/api/lens/987654").await.1,
        ),
        (
            "GET /api/agent/parts/987654/gdt",
            StatusCode::NOT_FOUND,
            get(&state, "/api/agent/parts/987654/gdt").await.1,
        ),
        (
            "GET /api/agent/parts/987654/profile",
            StatusCode::NOT_FOUND,
            get(&state, "/api/agent/parts/987654/profile").await.1,
        ),
        (
            "GET /api/agent/faces/987654",
            StatusCode::NOT_FOUND,
            get(&state, "/api/agent/faces/987654").await.1,
        ),
    ];
    for (what, _expected, body) in &cases {
        assert!(
            body["error_code"].is_string(),
            "{what}: no typed body; got {body}"
        );
        assert!(
            body["error"].as_str().is_some_and(|e| !e.is_empty()),
            "{what}: {body}"
        );
    }
    // The status codes are the ones these routes always answered.
    for (uri, expected) in [
        (
            "/api/timeline/history/not-a-branch",
            StatusCode::BAD_REQUEST,
        ),
        ("/api/lens/987654", StatusCode::NOT_FOUND),
        ("/api/agent/faces/987654", StatusCode::NOT_FOUND),
    ] {
        let (status, body) = get(&state, uri).await;
        assert_eq!(status, expected, "{uri}: {body}");
    }
}

/// (d) DISCONNECTION GATE — the agent can actually reach the rename route.
///
/// `rename_part_by_uuid` was built, correct, routed, auth-guarded and covered
/// by `rename_endpoint_persists_name_into_kernel_snapshot` — and unreachable by
/// an agent, because no MCP tool called it. That is the fifteenth instance of
/// this repo's oldest failure class, and it had a visible cost: every
/// solid-producing tool takes a `name` EXCEPT `boolean`, whose result merely
/// inherits its base's name. So the throwaway cutters were called
/// `circlip_groove_cutter` while the piston they cut was called `solid_11`, and
/// the model tree could not say what the part was because nothing could tell it.
///
/// A registry entry alone would NOT have caught this: the ontology drift gate
/// compares two classification tables, and a tool can be classified on both
/// surfaces while its handler talks to nothing. This asserts the PRODUCTION CALL
/// SITE — that the shipped handler names the route the backend actually serves.
#[test]
fn part_rename_tool_calls_the_rename_route() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("roshera-mcp")
        .join("src")
        .join("tools")
        .join("modify.ts");
    let source = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read the MCP modify tools at {}: {e}. A gate that checked \
             nothing must not be mistaken for a gate that passed.",
            path.display()
        )
    });

    assert!(
        source.contains("\"part_rename\""),
        "roshera-mcp must expose a part_rename tool — without it the rename \
         route is reachable only from the frontend and an agent has no way to \
         say what the geometry it just built IS"
    );
    // The exact path the router serves. If either side moves, this fails.
    assert!(
        source.contains("/api/parts/uuid/${encodeURIComponent(object_uuid)}/name"),
        "part_rename must POST to /api/parts/uuid/{{uuid}}/name — the route \
         registered in main.rs. A tool that exists but calls nothing is the \
         disconnection this gate is for."
    );
}
