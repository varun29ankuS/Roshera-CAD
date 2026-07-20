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

/// The five benches + core (spec §Layer 1; open-Q1 resolution: GD&T lives
/// inside the drawing bench). The registry MUST classify every tool into
/// exactly one of these.
const ALLOWED_BENCHES: &[&str] = &[
    "core", "sketch", "assembly", "drawing", "analysis", "labels",
];

/// The complete set of tool names the MCP server exposes today (90 tools).
///
/// Extracted verbatim from `roshera-mcp/src/tools/*.ts` +
/// `roshera-mcp/src/core.ts` `server.tool(…)` / `server.registerTool(…)`
/// registrations. This is the COMPLETENESS PIN: it is intentionally an
/// explicit hand-list, not a glob, so a new MCP tool must be added here AND
/// given a registry entry in the same change — otherwise this test goes red.
const MCP_TOOL_NAMES: &[&str] = &[
    // assembly.ts
    "assembly_add_instance",
    "assembly_certify",
    "assembly_create",
    "assembly_dof",
    "assembly_drag",
    "assembly_interference",
    "assembly_list_instances",
    "assembly_mate",
    "assembly_solve",
    "assembly_transform_instance",
    "assembly_verify",
    "assembly_view",
    // timeline.ts
    "bind_parameter_name",
    "clear_timeline",
    "rebuild_certificate",
    "timeline_mould",
    "timeline_scrub",
    // blackboard.ts
    "blackboard_add_entry",
    "blackboard_clear",
    "blackboard_edit_entry",
    "blackboard_list",
    // modify.ts
    "boolean",
    "boolean_many",
    "chamfer_edges",
    "clear_parts",
    "delete_part",
    "drill_pattern",
    "fillet_edges",
    "shell",
    "transform",
    // create.ts
    "create_box",
    "create_cone",
    "create_cylinder",
    "create_sketch",
    "create_sphere",
    "nurbs_loft",
    "plane_from_face",
    "revolve",
    "sketch_add_shape",
    "sketch_extrude",
    "sketch_points",
    // inspect.ts
    "document_units",
    "get_face",
    "get_part",
    "get_revolve_profile",
    "list_parts",
    "mass_properties",
    "part_distance",
    "part_features",
    "select_edge",
    "select_face",
    "set_part_color",
    "verify_claim",
    // io.ts
    "drawing_export_sheet",
    "export_part",
    "import_step",
    "make_drawing",
    // drawing.ts
    "drawing_query",
    "drawing_read_semantics",
    // perception.ts
    "dimension_part",
    "get_pointer",
    "ground_truth",
    "measure_faces",
    "occupancy_view",
    "part_coverage",
    "render_part",
    "scene_view",
    "section_view",
    "verify_part",
    // gdt.ts
    "gdt_datum",
    "gdt_fcf",
    "gdt_report",
    // labels.ts
    "label_create",
    "label_delete",
    "label_list",
    "label_rename",
    "label_resolve",
    "propose_labels",
    // psketch.ts
    "psketch_add_entity",
    "psketch_begin",
    "psketch_certify",
    "psketch_constrain",
    "psketch_dof",
    "psketch_extrude",
    "psketch_op",
    "psketch_revolve",
    "psketch_solve",
    // queries.ts
    "point_query",
    "ray_query",
    "region_query",
];

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

    let missing: Vec<&&str> = MCP_TOOL_NAMES
        .iter()
        .filter(|n| !names.contains(**n))
        .collect();
    assert!(
        missing.is_empty(),
        "registry is missing entries for MCP tools exposed today: {missing:?}"
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
