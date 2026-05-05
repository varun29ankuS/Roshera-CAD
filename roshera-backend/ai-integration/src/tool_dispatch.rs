//! Tool dispatch layer: bridges LLM tool_use responses to engine-executable commands.
//!
//! When the LLM returns a `tool_use` content block (e.g., `{"type":"tool_use","name":"create_box",
//! "input":{"width":10.0,"height":5.0,"depth":3.0}}`), this module validates the arguments against
//! known tool schemas and converts them into `ParsedCommand` or `shared_types::Command` that the
//! existing executor pipeline can handle.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::providers::{CommandIntent, ParsedCommand, ProviderError};
use geometry_engine::primitives::tool_schema_generator::{ToolSchema, ToolTier};

/// A parsed tool_use block from the LLM response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolUseBlock {
    /// Tool call ID (for correlating results back to the LLM)
    pub id: String,
    /// Tool name (e.g., "create_box", "boolean_union", "query_geometry")
    pub name: String,
    /// Tool input arguments as JSON
    pub input: Value,
}

/// Result of dispatching a tool call — either a geometry command or a query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DispatchResult {
    /// A geometry-modifying command that should be executed by CommandExecutor
    Command(ParsedCommand),
    /// A query that returns information without modifying geometry
    Query(ParsedCommand),
    /// The tool call produced a text response directly (e.g., help, listing)
    TextResponse(String),
}

/// Dispatch a tool_use block from the LLM into an executable command.
///
/// Validates the tool name exists in the registered schema set, extracts and validates
/// arguments, and produces a `DispatchResult` that the executor pipeline can handle.
pub fn dispatch_tool_call(tool_use: &ToolUseBlock) -> Result<DispatchResult, ProviderError> {
    let name = tool_use.name.as_str();
    let input = &tool_use.input;

    match name {
        // --- Primitive creation ---
        "create_box" => dispatch_create_primitive("box", input, &["width", "height", "depth"]),
        "create_cylinder" => dispatch_create_primitive("cylinder", input, &["radius", "height"]),
        "create_sphere" => dispatch_create_primitive("sphere", input, &["radius"]),
        "create_cone" => {
            dispatch_create_primitive("cone", input, &["bottom_radius", "top_radius", "height"])
        }
        "create_torus" => {
            dispatch_create_primitive("torus", input, &["major_radius", "minor_radius"])
        }

        // --- Boolean operations ---
        "boolean_union" | "boolean_intersection" | "boolean_difference" => {
            dispatch_boolean(name, input)
        }

        // --- Transform ---
        "transform_object" => dispatch_transform(input),

        // --- Modeling operations ---
        "extrude" => dispatch_operation("extrude", input),
        "revolve" => dispatch_operation("revolve", input),
        "chamfer" => dispatch_operation("chamfer", input),
        "fillet" => dispatch_operation("fillet", input),

        // --- Queries ---
        "query_geometry" => dispatch_query("query_geometry", input),
        "export_stl" => dispatch_export("stl", input),
        "export_obj" => dispatch_export("obj", input),

        // --- Datum authoring (slice 4a) ---
        "create_datum_plane" => dispatch_create_datum_plane(input),
        "create_datum_axis" => dispatch_create_datum_axis(input),
        "create_datum_point" => dispatch_create_datum_point(input),

        _ => Err(ProviderError::InvalidInput(format!(
            "Unknown tool: '{}'. Available tools depend on the active tier. \
             Core tools: create_box, create_cylinder, create_sphere, create_cone, \
             create_torus, boolean_union, boolean_difference, boolean_intersection, \
             transform_object, query_geometry, export_stl, \
             create_datum_plane, create_datum_axis, create_datum_point",
            name
        ))),
    }
}

/// Validate that all required parameters are present and are numbers.
fn validate_required_numbers(
    input: &Value,
    required: &[&str],
) -> Result<HashMap<String, Value>, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let mut params = HashMap::new();

    for &field in required {
        let val = obj.get(field).ok_or_else(|| {
            ProviderError::InvalidInput(format!(
                "Missing required parameter '{}'. Expected: {:?}",
                field, required
            ))
        })?;

        if !val.is_number() {
            return Err(ProviderError::InvalidInput(format!(
                "Parameter '{}' must be a number, got: {}",
                field, val
            )));
        }

        params.insert(field.to_string(), val.clone());
    }

    // Include any optional parameters that were provided
    for (key, val) in obj {
        if !params.contains_key(key.as_str()) {
            params.insert(key.clone(), val.clone());
        }
    }

    Ok(params)
}

fn dispatch_create_primitive(
    shape: &str,
    input: &Value,
    required: &[&str],
) -> Result<DispatchResult, ProviderError> {
    let params = validate_required_numbers(input, required)?;

    // Validate positive dimensions
    for &field in required {
        if let Some(val) = params.get(field) {
            if let Some(n) = val.as_f64() {
                if n <= 0.0 {
                    return Err(ProviderError::InvalidInput(format!(
                        "Parameter '{}' must be positive, got: {}",
                        field, n
                    )));
                }
            }
        }
    }

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: format!("create_{}", shape),
        intent: CommandIntent::CreatePrimitive {
            shape: shape.to_string(),
        },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_boolean(operation: &str, input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let object_a = obj.get("object_a").ok_or_else(|| {
        ProviderError::InvalidInput("Missing required parameter 'object_a'".to_string())
    })?;
    let object_b = obj.get("object_b").ok_or_else(|| {
        ProviderError::InvalidInput("Missing required parameter 'object_b'".to_string())
    })?;

    let op_type = operation
        .strip_prefix("boolean_")
        .unwrap_or(operation)
        .to_string();

    let mut params = HashMap::new();
    params.insert("object_a".to_string(), object_a.clone());
    params.insert("object_b".to_string(), object_b.clone());

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: operation.to_string(),
        intent: CommandIntent::BooleanOperation { operation: op_type },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_transform(input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let object_id = obj.get("object_id").ok_or_else(|| {
        ProviderError::InvalidInput("Missing required parameter 'object_id'".to_string())
    })?;

    let op = obj
        .get("operation")
        .and_then(|v| v.as_str())
        .unwrap_or("translate")
        .to_string();

    let mut params: HashMap<String, Value> =
        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    params.insert("object_id".to_string(), object_id.clone());

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: "transform_object".to_string(),
        intent: CommandIntent::Transform { operation: op },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_operation(op_name: &str, input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let params: HashMap<String, Value> = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    let intent = match op_name {
        "extrude" => CommandIntent::Extrude {
            target: obj
                .get("target_id")
                .and_then(|v| v.as_str())
                .map(String::from),
        },
        _ => CommandIntent::Modify {
            target: obj
                .get("target_id")
                .and_then(|v| v.as_str())
                .unwrap_or("last")
                .to_string(),
            operation: op_name.to_string(),
            parameters: input.clone(),
        },
    };

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: op_name.to_string(),
        intent,
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_query(query_type: &str, input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let params: HashMap<String, Value> = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    Ok(DispatchResult::Query(ParsedCommand {
        original_text: query_type.to_string(),
        intent: CommandIntent::Query {
            target: query_type.to_string(),
        },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

/// Validate and extract a non-empty string field.
fn require_non_empty_string(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<String, ProviderError> {
    let val = obj.get(field).ok_or_else(|| {
        ProviderError::InvalidInput(format!("Missing required parameter '{}'", field))
    })?;
    let s = val.as_str().ok_or_else(|| {
        ProviderError::InvalidInput(format!(
            "Parameter '{}' must be a string, got: {}",
            field, val
        ))
    })?;
    if s.trim().is_empty() {
        return Err(ProviderError::InvalidInput(format!(
            "Parameter '{}' must be a non-empty string",
            field
        )));
    }
    Ok(s.to_string())
}

/// Validate and extract a `{x, y, z}` numeric triple.
fn require_xyz(
    obj: &serde_json::Map<String, Value>,
    field: &str,
) -> Result<[f64; 3], ProviderError> {
    let val = obj.get(field).ok_or_else(|| {
        ProviderError::InvalidInput(format!("Missing required parameter '{}'", field))
    })?;
    let inner = val.as_object().ok_or_else(|| {
        ProviderError::InvalidInput(format!(
            "Parameter '{}' must be an object with x/y/z fields",
            field
        ))
    })?;
    let mut out = [0.0_f64; 3];
    for (i, axis) in ["x", "y", "z"].iter().enumerate() {
        let v = inner.get(*axis).ok_or_else(|| {
            ProviderError::InvalidInput(format!(
                "Parameter '{}.{}' is required",
                field, axis
            ))
        })?;
        out[i] = v.as_f64().ok_or_else(|| {
            ProviderError::InvalidInput(format!(
                "Parameter '{}.{}' must be a number, got: {}",
                field, axis, v
            ))
        })?;
    }
    Ok(out)
}

fn dispatch_create_datum_plane(input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let name = require_non_empty_string(obj, "name")?;
    let origin = require_xyz(obj, "origin")?;
    let normal = require_xyz(obj, "normal")?;

    // Reject zero-length normals — they cannot define a plane and the
    // kernel would otherwise fall through into a NaN-producing path
    // when the executor normalizes.
    let normal_len_sq = normal[0] * normal[0] + normal[1] * normal[1] + normal[2] * normal[2];
    if normal_len_sq < f64::EPSILON {
        return Err(ProviderError::InvalidInput(
            "Parameter 'normal' must have non-zero length".to_string(),
        ));
    }

    let mut params = HashMap::new();
    params.insert("name".to_string(), Value::String(name));
    params.insert(
        "origin".to_string(),
        serde_json::json!({ "x": origin[0], "y": origin[1], "z": origin[2] }),
    );
    params.insert(
        "normal".to_string(),
        serde_json::json!({ "x": normal[0], "y": normal[1], "z": normal[2] }),
    );

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: "create_datum_plane".to_string(),
        intent: CommandIntent::Create {
            object_type: "datum_plane".to_string(),
            parameters: input.clone(),
        },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_create_datum_axis(input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let name = require_non_empty_string(obj, "name")?;
    let origin = require_xyz(obj, "origin")?;
    let direction = require_non_empty_string(obj, "direction")?;
    let direction_lc = direction.to_ascii_lowercase();
    if !matches!(direction_lc.as_str(), "x" | "y" | "z") {
        return Err(ProviderError::InvalidInput(format!(
            "Parameter 'direction' must be one of \"x\", \"y\", \"z\"; got: {:?}",
            direction
        )));
    }

    let mut params = HashMap::new();
    params.insert("name".to_string(), Value::String(name));
    params.insert(
        "origin".to_string(),
        serde_json::json!({ "x": origin[0], "y": origin[1], "z": origin[2] }),
    );
    params.insert("direction".to_string(), Value::String(direction_lc));

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: "create_datum_axis".to_string(),
        intent: CommandIntent::Create {
            object_type: "datum_axis".to_string(),
            parameters: input.clone(),
        },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_create_datum_point(input: &Value) -> Result<DispatchResult, ProviderError> {
    let obj = input.as_object().ok_or_else(|| {
        ProviderError::InvalidInput("Tool input must be a JSON object".to_string())
    })?;

    let name = require_non_empty_string(obj, "name")?;
    let position = require_xyz(obj, "position")?;

    let mut params = HashMap::new();
    params.insert("name".to_string(), Value::String(name));
    params.insert(
        "position".to_string(),
        serde_json::json!({ "x": position[0], "y": position[1], "z": position[2] }),
    );

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: "create_datum_point".to_string(),
        intent: CommandIntent::Create {
            object_type: "datum_point".to_string(),
            parameters: input.clone(),
        },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

fn dispatch_export(format: &str, input: &Value) -> Result<DispatchResult, ProviderError> {
    let empty_map = serde_json::Map::new();
    let obj = input.as_object().unwrap_or(&empty_map);

    let params: HashMap<String, Value> = obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();

    Ok(DispatchResult::Command(ParsedCommand {
        original_text: format!("export_{}", format),
        intent: CommandIntent::Export {
            format: format.to_string(),
            options: input.clone(),
        },
        parameters: params,
        confidence: 1.0,
        language: "en".to_string(),
    }))
}

/// Build the list of Anthropic-format tool definitions for a given tier.
///
/// This is the bridge between the geometry engine's tool schema generator
/// and the Anthropic API's expected tool format.
pub fn tool_definitions_for_tier(tier: ToolTier) -> Vec<Value> {
    let schemas = geometry_engine::primitives::tool_schema_generator::tiered_tool_schemas(tier);
    geometry_engine::primitives::tool_schema_generator::to_anthropic_tools(&schemas)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_dispatch_create_box() {
        let tool_use = ToolUseBlock {
            id: "call_1".to_string(),
            name: "create_box".to_string(),
            input: json!({"width": 10.0, "height": 5.0, "depth": 3.0}),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                assert!(
                    matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "box")
                );
                assert_eq!(cmd.parameters["width"], json!(10.0));
                assert_eq!(cmd.parameters["height"], json!(5.0));
                assert_eq!(cmd.parameters["depth"], json!(3.0));
                assert_eq!(cmd.confidence, 1.0);
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_create_sphere() {
        let tool_use = ToolUseBlock {
            id: "call_2".to_string(),
            name: "create_sphere".to_string(),
            input: json!({"radius": 5.0}),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                assert!(
                    matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "sphere")
                );
                assert_eq!(cmd.parameters["radius"], json!(5.0));
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_missing_required_param() {
        let tool_use = ToolUseBlock {
            id: "call_3".to_string(),
            name: "create_box".to_string(),
            input: json!({"width": 10.0}), // missing height and depth
        };

        let result = dispatch_tool_call(&tool_use);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("height"));
    }

    #[test]
    fn test_dispatch_negative_dimension() {
        let tool_use = ToolUseBlock {
            id: "call_4".to_string(),
            name: "create_cylinder".to_string(),
            input: json!({"radius": -5.0, "height": 10.0}),
        };

        let result = dispatch_tool_call(&tool_use);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("positive"));
    }

    #[test]
    fn test_dispatch_boolean_union() {
        let tool_use = ToolUseBlock {
            id: "call_5".to_string(),
            name: "boolean_union".to_string(),
            input: json!({"object_a": "solid_0", "object_b": "solid_1"}),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                assert!(
                    matches!(cmd.intent, CommandIntent::BooleanOperation { ref operation } if operation == "union")
                );
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_query_geometry() {
        let tool_use = ToolUseBlock {
            id: "call_6".to_string(),
            name: "query_geometry".to_string(),
            input: json!({"solid_id": 0}),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        assert!(matches!(result, DispatchResult::Query(_)));
    }

    #[test]
    fn test_dispatch_unknown_tool() {
        let tool_use = ToolUseBlock {
            id: "call_7".to_string(),
            name: "nonexistent_tool".to_string(),
            input: json!({}),
        };

        let result = dispatch_tool_call(&tool_use);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Unknown tool"));
    }

    #[test]
    fn test_dispatch_transform() {
        let tool_use = ToolUseBlock {
            id: "call_8".to_string(),
            name: "transform_object".to_string(),
            input: json!({
                "object_id": "solid_0",
                "operation": "translate",
                "x": 10.0, "y": 0.0, "z": 0.0
            }),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                assert!(
                    matches!(cmd.intent, CommandIntent::Transform { ref operation } if operation == "translate")
                );
                assert_eq!(cmd.parameters["x"], json!(10.0));
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_export_stl() {
        let tool_use = ToolUseBlock {
            id: "call_9".to_string(),
            name: "export_stl".to_string(),
            input: json!({"solid_id": 0, "file_path": "output.stl"}),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                assert!(
                    matches!(cmd.intent, CommandIntent::Export { ref format, .. } if format == "stl")
                );
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_tool_definitions_for_tier() {
        let tools = tool_definitions_for_tier(ToolTier::Tier1);
        assert!(!tools.is_empty());

        // Every tool definition must have name, description, input_schema
        for tool in &tools {
            assert!(tool.get("name").is_some());
            assert!(tool.get("description").is_some());
            assert!(tool.get("input_schema").is_some());
        }
    }

    #[test]
    fn test_dispatch_create_datum_plane() {
        let tool_use = ToolUseBlock {
            id: "call_datum_1".to_string(),
            name: "create_datum_plane".to_string(),
            input: json!({
                "name": "TopRef",
                "origin": { "x": 0.0, "y": 0.0, "z": 10.0 },
                "normal": { "x": 0.0, "y": 0.0, "z": 1.0 },
            }),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                match &cmd.intent {
                    CommandIntent::Create {
                        object_type,
                        parameters: _,
                    } => assert_eq!(object_type, "datum_plane"),
                    other => panic!("expected CommandIntent::Create, got {:?}", other),
                }
                assert_eq!(cmd.parameters["name"], json!("TopRef"));
                assert_eq!(cmd.parameters["origin"]["z"], json!(10.0));
                assert_eq!(cmd.parameters["normal"]["z"], json!(1.0));
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_create_datum_plane_rejects_zero_normal() {
        let tool_use = ToolUseBlock {
            id: "call_datum_2".to_string(),
            name: "create_datum_plane".to_string(),
            input: json!({
                "name": "Bad",
                "origin": { "x": 0.0, "y": 0.0, "z": 0.0 },
                "normal": { "x": 0.0, "y": 0.0, "z": 0.0 },
            }),
        };
        let err = dispatch_tool_call(&tool_use).unwrap_err().to_string();
        assert!(err.contains("non-zero"));
    }

    #[test]
    fn test_dispatch_create_datum_plane_rejects_empty_name() {
        let tool_use = ToolUseBlock {
            id: "call_datum_3".to_string(),
            name: "create_datum_plane".to_string(),
            input: json!({
                "name": "   ",
                "origin": { "x": 0.0, "y": 0.0, "z": 0.0 },
                "normal": { "x": 0.0, "y": 0.0, "z": 1.0 },
            }),
        };
        let err = dispatch_tool_call(&tool_use).unwrap_err().to_string();
        assert!(err.contains("non-empty"));
    }

    #[test]
    fn test_dispatch_create_datum_axis_canonical() {
        let tool_use = ToolUseBlock {
            id: "call_datum_4".to_string(),
            name: "create_datum_axis".to_string(),
            input: json!({
                "name": "ProbeAxis",
                "origin": { "x": 1.0, "y": 2.0, "z": 3.0 },
                "direction": "Y",
            }),
        };
        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                match &cmd.intent {
                    CommandIntent::Create {
                        object_type,
                        parameters: _,
                    } => assert_eq!(object_type, "datum_axis"),
                    other => panic!("expected CommandIntent::Create, got {:?}", other),
                }
                // Direction is canonicalized to lowercase.
                assert_eq!(cmd.parameters["direction"], json!("y"));
                assert_eq!(cmd.parameters["origin"]["x"], json!(1.0));
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_create_datum_axis_rejects_non_canonical() {
        let tool_use = ToolUseBlock {
            id: "call_datum_5".to_string(),
            name: "create_datum_axis".to_string(),
            input: json!({
                "name": "Diagonal",
                "origin": { "x": 0.0, "y": 0.0, "z": 0.0 },
                "direction": "diagonal",
            }),
        };
        let err = dispatch_tool_call(&tool_use).unwrap_err().to_string();
        assert!(err.contains("direction"));
    }

    #[test]
    fn test_dispatch_create_datum_point() {
        let tool_use = ToolUseBlock {
            id: "call_datum_6".to_string(),
            name: "create_datum_point".to_string(),
            input: json!({
                "name": "P1",
                "position": { "x": 4.0, "y": 5.0, "z": 6.0 },
            }),
        };
        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                match &cmd.intent {
                    CommandIntent::Create {
                        object_type,
                        parameters: _,
                    } => assert_eq!(object_type, "datum_point"),
                    other => panic!("expected CommandIntent::Create, got {:?}", other),
                }
                assert_eq!(cmd.parameters["name"], json!("P1"));
                assert_eq!(cmd.parameters["position"]["y"], json!(5.0));
            }
            _ => panic!("Expected Command dispatch"),
        }
    }

    #[test]
    fn test_dispatch_create_datum_point_rejects_missing_position() {
        let tool_use = ToolUseBlock {
            id: "call_datum_7".to_string(),
            name: "create_datum_point".to_string(),
            input: json!({ "name": "P1" }),
        };
        let err = dispatch_tool_call(&tool_use).unwrap_err().to_string();
        assert!(err.contains("position"));
    }

    #[test]
    fn test_dispatch_cone_all_params() {
        let tool_use = ToolUseBlock {
            id: "call_10".to_string(),
            name: "create_cone".to_string(),
            input: json!({"bottom_radius": 5.0, "top_radius": 2.0, "height": 10.0}),
        };

        let result = dispatch_tool_call(&tool_use).unwrap();
        match result {
            DispatchResult::Command(cmd) => {
                assert!(
                    matches!(cmd.intent, CommandIntent::CreatePrimitive { ref shape } if shape == "cone")
                );
                assert_eq!(cmd.parameters["bottom_radius"], json!(5.0));
                assert_eq!(cmd.parameters["top_radius"], json!(2.0));
            }
            _ => panic!("Expected Command dispatch"),
        }
    }
}
