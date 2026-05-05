//! Capability discovery endpoint.
//!
//! Roshera positions itself as an agent runtime for geometry. An agent
//! (LLM, script, human) needs to discover, without prior knowledge of the
//! source, what shapes can be created and what operations are available
//! along with the exact parameter contract each accepts. `GET
//! /api/capabilities` returns a single self-describing JSON document
//! covering every primitive and operation the kernel exposes through the
//! HTTP surface.
//!
//! The response is intentionally a flat document of concrete examples
//! rather than a JSON Schema graph: agents have already proven better at
//! pattern-matching example payloads than at resolving `$ref`/`$defs`
//! chains. Each primitive lists the exact required parameter keys (the
//! canonical source of truth used by `main.rs::create_geometry` and
//! `protocol::geometry_handlers::handle_create_primitive`) so the surface
//! is identical whether driven over REST or WebSocket.
//!
//! When primitives or operations are added, update this document in the
//! same commit — it is the discovery contract; drift here means agents
//! fly blind.
//!
//! # Stability
//! - Versioned via `kernel_version` in the response.
//! - Backwards-compatible additions (new primitives, new optional params)
//!   bump the patch version. Removed parameters or removed shapes bump
//!   the minor version.

use crate::error_catalog::ErrorCode;
use crate::AppState;
use axum::extract::State;
use axum::response::Json;
use serde_json::{json, Value};

/// Returns the capability document. The catalog (primitives, operations,
/// endpoints, error codes) is static, but the embedded `runtime` block
/// reflects deployment-time configuration — currently whether an LLM
/// provider key was found at server start. Agents are expected to call
/// this once per session and memoise the catalog portion; the runtime
/// flags are cheap to re-read if they want fresh state.
pub async fn capabilities(State(state): State<AppState>) -> Json<Value> {
    Json(build_capabilities(state.ai_configured))
}

fn build_capabilities(ai_configured: bool) -> Value {
    json!({
        "kernel": "roshera-geometry-engine",
        "kernel_version": env!("CARGO_PKG_VERSION"),
        "discovery_version": "1.0.0",
        "runtime": {
            "ai_configured": ai_configured,
        },
        "description": "Agent-readable surface for the Roshera B-Rep kernel. \
            Every primitive and operation listed here is reachable via the \
            documented HTTP endpoint and produces a real solid in the \
            shared kernel model.",
        "conventions": {
            "units": "Lengths are in model units (unitless). The frontend \
                displays them as millimetres by default; conversion is a \
                client concern, not a kernel one.",
            "axes": "Right-handed. Cylinders, cones, and tori are oriented \
                along +Z by default; placement is controlled by the \
                `position` field, not by parameters.",
            "ids": "Objects returned in responses are addressed by UUIDv4 \
                strings. Pass these UUIDs back as inputs to subsequent \
                operations.",
            "errors": "Every error response carries `success: false`, a \
                stable `error_code` (snake_case identifier from the \
                catalog under `error_codes` below), a human-readable \
                `error` string, and a `retryable` boolean. Optional \
                `hint` and `details` fields may also be present. \
                Agents must pattern-match on `error_code`, never on \
                the prose `error`. The kernel never silently \
                substitutes default dimensions.",
            "idempotency": "Every mutating endpoint (POST/PUT/PATCH/DELETE) \
                honours an optional `Idempotency-Key` request header. \
                Sending the same key + same body twice replays the \
                cached response with `Idempotency-Replayed: true`; \
                same key + different body returns 409 CONFLICT. 5xx \
                responses are never cached so transient kernel errors \
                stay retryable. Cache window: 24 hours. Use a fresh \
                UUID per logical command.",
            "transactions": "Multi-step plans may be wrapped in an atomic \
                transaction so partial work doesn't leak into the model on \
                failure. Open one with `POST /api/tx/begin`, then quote the \
                returned `tx_id` in the `X-Roshera-Tx-Id` header on each \
                subsequent mutation. `POST /api/tx/{id}/commit` makes the \
                tracked solids permanent; `POST /api/tx/{id}/rollback` \
                removes every solid created under the transaction. \
                Transactions auto-expire after 1 hour of inactivity. \
                Errors surface as `transaction_not_found` (NOT_FOUND, \
                non-retryable) or `transaction_not_active` (CONFLICT, \
                non-retryable). Header is opt-in: omitting it preserves \
                pre-transaction behaviour.",
            "branches": "Agents claim sandbox branches off `main` so \
                concurrent agents never collide in the immutable event \
                log. `POST /api/branches` creates a branch (optional \
                `agent_id` tag becomes the recorded author); `GET \
                /api/branches` lists every branch with its state, agent \
                tag, parent, and event count; `GET /api/branches/{id}` \
                returns one; `DELETE /api/branches/{id}` flips the \
                branch's state to `abandoned` (events stay for \
                forensics; `main` is rejected); `POST /api/branches/\
                {id}/merge` folds the branch into a target (default \
                `main`) using a `strategy` of `fast-forward` (default), \
                `three-way`, or `squash`. Branch IDs on the wire are \
                either the literal `\"main\"` or a UUIDv4 string. \
                Errors surface as `branch_not_found` (NOT_FOUND), \
                `branch_invalid_state` (CONFLICT, e.g. abandoning \
                `main` or re-abandoning a merged branch), or \
                `branch_merge_conflict` (CONFLICT). Mutation routing \
                — landing geometry ops on the agent's branch instead \
                of the trunk model — is a separate kernel layer that \
                this surface does not yet enforce; the branch \
                lifecycle and event-log isolation it exposes are \
                correct and useful on their own.",
            "frame": "Multimodal agents can fetch a server-rendered PNG of \
                the live scene with `GET /api/frame`. The kernel \
                tessellates every solid, projects with an auto-fit \
                isometric camera, rasterizes on the CPU with Lambert \
                shading + per-solid hue, and returns `image/png`. \
                Optional query parameters: `width` and `height` (1-2048, \
                default 1024x768); `eye_x`/`eye_y`/`eye_z` and \
                `target_x`/`target_y`/`target_z` to override the camera \
                (all six required together to take effect); `fov_deg` \
                (1-179, default 35). Empty scenes return a solid \
                background image so the response is always a valid PNG.",
            "ai_streaming": "POST /api/ai/command/stream returns an SSE \
                stream of LLM tokens at provider cadence (~30/s for \
                Claude Sonnet). Frames in order: `event: start` with \
                `{command, session_id}`; then a sequence of `event: \
                token` frames each carrying `{text}` for one delta; \
                terminated by `event: complete` with `{text, \
                session_id, user_id}` containing the full concatenated \
                response. Failures surface as a single `event: error` \
                frame with `{error, stage}` and the connection closes. \
                If the client disconnects mid-stream the upstream HTTP \
                request to the LLM is dropped immediately so no \
                further tokens are billed.",
            "ai_configuration": "AI routes (`/api/ai/command`, \
                `/api/ai/command/stream`) require an LLM provider key \
                set in the server environment at startup — currently \
                `ANTHROPIC_API_KEY`. When unset, both routes refuse \
                with `503 ai_not_configured` (the streaming route \
                emits a single `event: error` frame carrying the same \
                JSON body and closes). There is no silent mock \
                fallback: a server with no key returns the structured \
                error every time so misconfiguration is visible to \
                agents and operators. The live state is published as \
                `runtime.ai_configured` on this same document, so \
                agents can branch their behaviour off the same GET \
                they use for capability discovery without provoking a \
                503. `GET /api/ai/status` exposes the same flag plus \
                the active provider name and remediation hint."
        },
        "primitives": primitives(),
        "operations": operations(),
        "endpoints": endpoints(),
        "error_codes": error_codes()
    })
}

/// Publish the closed error catalog so agents can preflight handlers
/// against every possible failure code without having to provoke each
/// one. The catalog is sourced from `error_catalog::ErrorCode::all()`
/// — there is one wire-format definition for both producer and
/// consumer, so drift between code and discovery is impossible.
fn error_codes() -> Value {
    let entries: Vec<Value> = ErrorCode::all()
        .iter()
        .map(|code| {
            json!({
                "code": code.as_str(),
                "http_status": code.status().as_u16(),
                "retryable": code.retryable(),
            })
        })
        .collect();
    Value::Array(entries)
}

fn primitives() -> Value {
    json!([
        {
            "shape_type": "box",
            "aliases": ["cube"],
            "endpoint": "POST /api/geometry",
            "required_parameters": {
                "width":  {"type": "number", "description": "Extent along X (>0)"},
                "height": {"type": "number", "description": "Extent along Y (>0)"},
                "depth":  {"type": "number", "description": "Extent along Z (>0)"}
            },
            "optional_parameters": {
                "position": {
                    "type": "array<number>",
                    "length": 3,
                    "default": [0.0, 0.0, 0.0],
                    "description": "World-space placement of the box's local origin"
                }
            },
            "example_request": {
                "shape_type": "box",
                "parameters": {"width": 10.0, "height": 10.0, "depth": 10.0},
                "position": [0.0, 0.0, 0.0]
            }
        },
        {
            "shape_type": "sphere",
            "endpoint": "POST /api/geometry",
            "required_parameters": {
                "radius": {"type": "number", "description": "Sphere radius (>0)"}
            },
            "optional_parameters": {
                "position": {
                    "type": "array<number>", "length": 3,
                    "default": [0.0, 0.0, 0.0],
                    "description": "World-space placement of the sphere centre"
                }
            },
            "example_request": {
                "shape_type": "sphere",
                "parameters": {"radius": 5.0},
                "position": [0.0, 0.0, 0.0]
            }
        },
        {
            "shape_type": "cylinder",
            "endpoint": "POST /api/geometry",
            "required_parameters": {
                "radius": {"type": "number", "description": "Cylinder radius (>0)"},
                "height": {"type": "number", "description": "Axial extent along +Z (>0)"}
            },
            "optional_parameters": {
                "position": {
                    "type": "array<number>", "length": 3,
                    "default": [0.0, 0.0, 0.0],
                    "description": "World-space placement of the bottom-cap centre"
                }
            },
            "example_request": {
                "shape_type": "cylinder",
                "parameters": {"radius": 5.0, "height": 10.0},
                "position": [0.0, 0.0, 0.0]
            }
        },
        {
            "shape_type": "cone",
            "endpoint": "POST /api/geometry",
            "required_parameters": {
                "radius": {"type": "number", "description": "Base radius (>0); apex is sharp (top_radius=0)"},
                "height": {"type": "number", "description": "Axial extent along +Z (>0)"}
            },
            "optional_parameters": {
                "position": {
                    "type": "array<number>", "length": 3,
                    "default": [0.0, 0.0, 0.0],
                    "description": "World-space placement of the base-cap centre"
                }
            },
            "example_request": {
                "shape_type": "cone",
                "parameters": {"radius": 5.0, "height": 10.0},
                "position": [0.0, 0.0, 0.0]
            }
        },
        {
            "shape_type": "torus",
            "endpoint": "POST /api/geometry",
            "required_parameters": {
                "major_radius": {"type": "number", "description": "Distance from torus centre to tube centre (>0)"},
                "minor_radius": {"type": "number", "description": "Tube cross-section radius (>0, < major_radius)"}
            },
            "optional_parameters": {
                "position": {
                    "type": "array<number>", "length": 3,
                    "default": [0.0, 0.0, 0.0],
                    "description": "World-space placement of the torus centre"
                }
            },
            "example_request": {
                "shape_type": "torus",
                "parameters": {"major_radius": 8.0, "minor_radius": 2.0},
                "position": [0.0, 0.0, 0.0]
            }
        }
    ])
}

fn operations() -> Value {
    json!([
        {
            "name": "boolean",
            "endpoint": "POST /api/geometry/boolean",
            "description": "Combine two or more solids via union, intersection, or difference.",
            "required_parameters": {
                "operation": {
                    "type": "string",
                    "enum": ["Union", "Intersection", "Difference"],
                    "description": "Set-theoretic operation to apply"
                },
                "objects": {
                    "type": "array<uuid>",
                    "min_length": 2,
                    "description": "Object UUIDs to combine. For Difference, the first \
                        object is the minuend and subsequent objects are subtracted."
                }
            },
            "optional_parameters": {
                "keep_originals": {
                    "type": "boolean",
                    "default": false,
                    "description": "When true, input objects remain in the session; otherwise they are removed."
                }
            },
            "example_request": {
                "operation": "Difference",
                "objects": ["a1b2c3d4-...-...-...", "e5f6g7h8-...-...-..."],
                "keep_originals": false
            }
        }
    ])
}

fn endpoints() -> Value {
    json!({
        "geometry": {
            "create": "POST /api/geometry",
            "boolean": "POST /api/geometry/boolean"
        },
        "introspection": {
            "capabilities": "GET /api/capabilities",
            "kernel_state": "GET /api/kernel/state",
            "frame":        "GET /api/frame",
            "health":       "GET /health"
        },
        "timeline": {
            "init": "POST /api/timeline/init",
            "history": "GET /api/timeline/history/{branch_id}",
            "undo": "POST /api/timeline/undo",
            "redo": "POST /api/timeline/redo"
        },
        "ai": {
            "command": "POST /api/ai/command",
            "command_stream": "POST /api/ai/command/stream",
            "status": "GET /api/ai/status"
        },
        "session": {
            "list": "GET /api/sessions",
            "create": "POST /api/sessions",
            "join": "POST /api/sessions/{id}/join",
            "leave": "POST /api/sessions/{id}/leave"
        },
        "transactions": {
            "begin":    "POST /api/tx/begin",
            "get":      "GET  /api/tx/{id}",
            "commit":   "POST /api/tx/{id}/commit",
            "rollback": "POST /api/tx/{id}/rollback"
        },
        "branches": {
            "list":   "GET    /api/branches",
            "create": "POST   /api/branches",
            "get":    "GET    /api/branches/{id}",
            "delete": "DELETE /api/branches/{id}",
            "merge":  "POST   /api/branches/{id}/merge"
        },
        "export": "POST /api/export"
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test that the document parses and has the expected top-level
    /// shape. If a future edit accidentally produces invalid JSON, this
    /// test fails immediately rather than at agent runtime.
    #[test]
    fn capability_document_has_required_sections() {
        let doc = build_capabilities(true);
        assert_eq!(doc["kernel"], "roshera-geometry-engine");
        assert!(doc["discovery_version"].is_string());
        assert!(doc["primitives"].is_array());
        assert!(doc["operations"].is_array());
        assert!(doc["endpoints"].is_object());
        assert!(
            doc["error_codes"].is_array(),
            "error_codes catalog must be published"
        );
        assert!(
            doc["runtime"].is_object(),
            "runtime block must be present so agents can read live config"
        );

        let prims = doc["primitives"].as_array().unwrap();
        assert_eq!(
            prims.len(),
            5,
            "expected 5 primitives (box, sphere, cylinder, cone, torus); \
             update this test in the same commit when adding a primitive"
        );

        // Every primitive must declare required_parameters and an
        // example_request — the agent contract depends on both.
        for p in prims {
            let shape = p["shape_type"].as_str().unwrap();
            assert!(
                p["required_parameters"].is_object(),
                "{shape}: missing required_parameters"
            );
            assert!(
                p["example_request"].is_object(),
                "{shape}: missing example_request"
            );
        }
    }

    /// `runtime.ai_configured` must reflect the AppState flag exactly so
    /// agents can branch on a single GET. A drift here would force them
    /// to also poll `/api/ai/status` or provoke a 503 to discover state.
    #[test]
    fn runtime_ai_configured_reflects_state() {
        let on = build_capabilities(true);
        let off = build_capabilities(false);
        assert_eq!(
            on["runtime"]["ai_configured"],
            serde_json::Value::Bool(true)
        );
        assert_eq!(
            off["runtime"]["ai_configured"],
            serde_json::Value::Bool(false)
        );
    }

    /// Lock in the exact required parameter keys for each primitive so
    /// that drift between this document and `main.rs::create_geometry` /
    /// `handle_create_primitive` produces a test failure rather than
    /// silent agent confusion.
    #[test]
    fn primitive_required_parameters_match_kernel_contract() {
        // The contract is the *set* of required keys, not their JSON
        // serialization order — `serde_json::Map` iteration order is an
        // implementation detail (alphabetical without `preserve_order`,
        // insertion order with it). Compare as sorted vectors so this
        // test stays a contract test, not an iteration-order assertion.
        let doc = build_capabilities(true);
        let prims = doc["primitives"].as_array().unwrap();
        for p in prims {
            let shape = p["shape_type"].as_str().unwrap();
            let req = p["required_parameters"].as_object().unwrap();
            let mut keys: Vec<&str> = req.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            match shape {
                "box" => assert_eq!(keys, vec!["depth", "height", "width"]),
                "sphere" => assert_eq!(keys, vec!["radius"]),
                "cylinder" => assert_eq!(keys, vec!["height", "radius"]),
                "cone" => assert_eq!(keys, vec!["height", "radius"]),
                "torus" => assert_eq!(keys, vec!["major_radius", "minor_radius"]),
                other => panic!("unexpected primitive shape_type: {other}"),
            }
        }
    }

    /// The published catalog must enumerate every variant in
    /// `ErrorCode`. Drift here means the agent gets a code at runtime
    /// it did not see during preflight — the exact failure mode
    /// `error_codes` exists to prevent.
    #[test]
    fn error_codes_catalog_covers_every_variant() {
        let doc = build_capabilities(true);
        let codes = doc["error_codes"].as_array().unwrap();
        assert_eq!(
            codes.len(),
            ErrorCode::all().len(),
            "discovery must publish exactly the codes the catalog defines"
        );
        for entry in codes {
            assert!(entry["code"].is_string());
            assert!(entry["http_status"].is_u64());
            assert!(entry["retryable"].is_boolean());
        }
        // Spot-check well-known codes.
        let by_code: std::collections::HashMap<&str, &serde_json::Value> = codes
            .iter()
            .map(|e| (e["code"].as_str().unwrap(), e))
            .collect();
        assert_eq!(by_code["missing_parameter"]["http_status"], 400);
        assert_eq!(by_code["missing_parameter"]["retryable"], false);
        assert_eq!(by_code["idempotency_key_reused"]["http_status"], 409);
        assert_eq!(by_code["kernel_error"]["retryable"], true);
    }
}
