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
    "core", "sketch", "assembly", "drawing", "analysis", "labels", "timeline",
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
