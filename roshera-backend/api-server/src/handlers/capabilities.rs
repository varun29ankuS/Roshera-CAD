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

use axum::response::Json;
use serde_json::{json, Value};

/// Returns the static capability document. Pure function — no kernel state
/// is read, so this is safe to serve under any auth context and cheap to
/// call repeatedly. Agents are expected to memoise the result for the
/// duration of a session.
pub async fn capabilities() -> Json<Value> {
    Json(build_capabilities())
}

fn build_capabilities() -> Value {
    json!({
        "kernel": "roshera-geometry-engine",
        "kernel_version": env!("CARGO_PKG_VERSION"),
        "discovery_version": "1.0.0",
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
            "errors": "Missing or non-numeric required parameters return \
                400 BAD_REQUEST with `{\"success\": false, \"error\": \
                \"missing or non-numeric parameter 'X'\"}`. The kernel \
                never silently substitutes default dimensions."
        },
        "primitives": primitives(),
        "operations": operations(),
        "endpoints": endpoints()
    })
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
            "health": "GET /health"
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
        let doc = build_capabilities();
        assert_eq!(doc["kernel"], "roshera-geometry-engine");
        assert!(doc["discovery_version"].is_string());
        assert!(doc["primitives"].is_array());
        assert!(doc["operations"].is_array());
        assert!(doc["endpoints"].is_object());

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

    /// Lock in the exact required parameter keys for each primitive so
    /// that drift between this document and `main.rs::create_geometry` /
    /// `handle_create_primitive` produces a test failure rather than
    /// silent agent confusion.
    #[test]
    fn primitive_required_parameters_match_kernel_contract() {
        let doc = build_capabilities();
        let prims = doc["primitives"].as_array().unwrap();
        for p in prims {
            let shape = p["shape_type"].as_str().unwrap();
            let req = p["required_parameters"].as_object().unwrap();
            let keys: Vec<&str> = req.keys().map(|k| k.as_str()).collect();
            match shape {
                "box" => assert_eq!(keys, vec!["width", "height", "depth"]),
                "sphere" => assert_eq!(keys, vec!["radius"]),
                "cylinder" => assert_eq!(keys, vec!["radius", "height"]),
                "cone" => assert_eq!(keys, vec!["radius", "height"]),
                "torus" => assert_eq!(keys, vec!["major_radius", "minor_radius"]),
                other => panic!("unexpected primitive shape_type: {other}"),
            }
        }
    }
}
