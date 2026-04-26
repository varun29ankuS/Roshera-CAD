//! LLM Tool Schema Generation from CAD Registries.
//!
//! Converts PrimitiveSchema and operation metadata into JSON Schema format
//! compatible with Anthropic's tool_use API and OpenAI's function calling.
//!
//! # Design
//!
//! Tool schemas are generated statically from the geometry engine's type system.
//! Tiered disclosure prevents context window bloat: Tier1 covers the most common
//! primitives and operations, Tier2 adds modeling operations, Tier3 includes all.
//!
//! Indexed access into schema parameter arrays is the canonical idiom — all
//! `arr[i]` sites use indices bounded by parameter list length validated at
//! schema construction. Matches the numerical-kernel pattern used in nurbs.rs.
#![allow(clippy::indexing_slicing)]

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::primitives::natural_language_schemas::{
    ParameterDataType, ParameterInfo, PrimitiveSchema,
};

/// Tool tier for context-aware schema disclosure.
///
/// Tier1 is always included. Higher tiers added when the LLM requests
/// capabilities beyond what's available, or when the user's prompt
/// implies complex operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolTier {
    /// Core primitives + boolean + transform (5 primitives, 3 operations)
    Tier1,
    /// Adds extrude, revolve, fillet, chamfer, shell
    Tier2,
    /// All registered primitives and operations
    Tier3,
}

/// A generated tool schema ready for LLM API submission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

/// Convert a PrimitiveSchema into an Anthropic-compatible tool definition.
///
/// Maps:
/// - `ParameterDataType::Number` → `{"type": "number"}`
/// - `ParameterDataType::Length` → `{"type": "number", "description": "...(mm)"}`
/// - `ParameterDataType::Angle` → `{"type": "number", "description": "...(degrees)"}`
/// - `ParameterDataType::Boolean` → `{"type": "boolean"}`
/// - `ParameterDataType::Choice` → `{"type": "string", "enum": [...]}`
/// - `ParameterDataType::Point3D` → `{"type": "object", "properties": {"x","y","z"}}`
/// - `ParameterDataType::Vector3D` → `{"type": "object", "properties": {"x","y","z"}}`
/// - `ParameterDataType::Text` → `{"type": "string"}`
pub fn primitive_to_tool_schema(schema: &PrimitiveSchema) -> ToolSchema {
    let mut properties = serde_json::Map::new();
    let mut required = Vec::new();

    for param in &schema.parameters {
        let prop = parameter_to_json_schema(param);
        properties.insert(param.name.clone(), prop);
        if param.required {
            required.push(Value::String(param.name.clone()));
        }
    }

    let input_schema = json!({
        "type": "object",
        "properties": properties,
        "required": required,
    });

    let description = format!("{}. {}", schema.description, schema.ai_explanation);

    ToolSchema {
        name: format!("create_{}", schema.id),
        description,
        input_schema,
    }
}

/// Convert a single ParameterInfo into a JSON Schema property.
fn parameter_to_json_schema(param: &ParameterInfo) -> Value {
    let base_desc = format!("{} — {}", param.description, param.effect_description);

    match &param.data_type {
        ParameterDataType::Number {
            precision: _,
            format_hint: _,
        } => {
            let mut schema = json!({
                "type": "number",
                "description": base_desc,
            });
            if let Some(min) = param.constraints.min {
                schema["minimum"] = json!(min);
            }
            if let Some(max) = param.constraints.max {
                schema["maximum"] = json!(max);
            }
            if param.constraints.integer_only {
                schema["type"] = json!("integer");
            }
            if let Some(ref default) = param.default_value {
                schema["default"] = default.clone();
            }
            schema
        }
        ParameterDataType::Length {
            default_unit,
            precision: _,
        } => {
            let desc = format!("{} (unit: {})", base_desc, default_unit);
            let mut schema = json!({
                "type": "number",
                "description": desc,
            });
            if param.constraints.positive_only {
                schema["exclusiveMinimum"] = json!(0);
            }
            if let Some(min) = param.constraints.min {
                schema["minimum"] = json!(min);
            }
            if let Some(max) = param.constraints.max {
                schema["maximum"] = json!(max);
            }
            if let Some(ref default) = param.default_value {
                schema["default"] = default.clone();
            }
            schema
        }
        ParameterDataType::Angle {
            default_unit,
            precision: _,
        } => {
            let desc = format!("{} (unit: {})", base_desc, default_unit);
            let mut schema = json!({
                "type": "number",
                "description": desc,
            });
            if let Some(min) = param.constraints.min {
                schema["minimum"] = json!(min);
            }
            if let Some(max) = param.constraints.max {
                schema["maximum"] = json!(max);
            }
            if let Some(ref default) = param.default_value {
                schema["default"] = default.clone();
            }
            schema
        }
        ParameterDataType::Boolean { .. } => {
            let mut schema = json!({
                "type": "boolean",
                "description": base_desc,
            });
            if let Some(ref default) = param.default_value {
                schema["default"] = default.clone();
            }
            schema
        }
        ParameterDataType::Choice { options } => {
            let enum_values: Vec<Value> = options
                .iter()
                .map(|o| Value::String(o.value.clone()))
                .collect();
            let option_descs: Vec<String> = options
                .iter()
                .map(|o| format!("'{}': {}", o.value, o.description))
                .collect();
            let desc = format!("{}. Options: {}", base_desc, option_descs.join("; "));
            let mut schema = json!({
                "type": "string",
                "description": desc,
                "enum": enum_values,
            });
            if let Some(ref default) = param.default_value {
                schema["default"] = default.clone();
            }
            schema
        }
        ParameterDataType::Point3D { coordinate_system } => {
            let desc = format!("{} ({} coordinates)", base_desc, coordinate_system);
            json!({
                "type": "object",
                "description": desc,
                "properties": {
                    "x": { "type": "number", "description": "X coordinate" },
                    "y": { "type": "number", "description": "Y coordinate" },
                    "z": { "type": "number", "description": "Z coordinate" },
                },
                "required": ["x", "y", "z"],
            })
        }
        ParameterDataType::Vector3D { normalized } => {
            let desc = if *normalized {
                format!("{} (unit vector, will be normalized)", base_desc)
            } else {
                base_desc
            };
            json!({
                "type": "object",
                "description": desc,
                "properties": {
                    "x": { "type": "number", "description": "X component" },
                    "y": { "type": "number", "description": "Y component" },
                    "z": { "type": "number", "description": "Z component" },
                },
                "required": ["x", "y", "z"],
            })
        }
        ParameterDataType::Text {
            max_length,
            pattern,
        } => {
            let mut schema = json!({
                "type": "string",
                "description": base_desc,
            });
            if let Some(max_len) = max_length {
                schema["maxLength"] = json!(max_len);
            }
            if let Some(ref pat) = pattern {
                schema["pattern"] = json!(pat);
            }
            schema
        }
    }
}

/// Generate a tool schema for a named CAD operation.
///
/// Operations are things like boolean, extrude, revolve, chamfer, etc.
/// They operate on existing geometry rather than creating new primitives.
pub fn operation_tool_schema(
    name: &str,
    description: &str,
    parameters: Vec<(&str, &str, Value)>, // (name, description, json_schema)
    required_params: Vec<&str>,
) -> ToolSchema {
    let mut properties = serde_json::Map::new();
    for (pname, pdesc, pschema) in &parameters {
        let mut schema = pschema.clone();
        if let Some(obj) = schema.as_object_mut() {
            obj.insert("description".to_string(), json!(pdesc));
        }
        properties.insert(pname.to_string(), schema);
    }

    let required: Vec<Value> = required_params
        .iter()
        .map(|s| Value::String(s.to_string()))
        .collect();

    ToolSchema {
        name: name.to_string(),
        description: description.to_string(),
        input_schema: json!({
            "type": "object",
            "properties": properties,
            "required": required,
        }),
    }
}

/// Build the standard set of geometry operation tool schemas.
///
/// These are hardcoded because operations have complex parameter types
/// (entity references, enums, nested options) that don't map from
/// PrimitiveSchema — they come from the operations module directly.
pub fn builtin_operation_schemas() -> Vec<ToolSchema> {
    vec![
        operation_tool_schema(
            "boolean_operation",
            "Perform a boolean operation (union, difference, intersection) between two solids",
            vec![
                ("operation", "Boolean operation type", json!({"type": "string", "enum": ["union", "difference", "intersection"]})),
                ("tool_solid_id", "ID of the tool solid", json!({"type": "integer"})),
                ("target_solid_id", "ID of the target solid", json!({"type": "integer"})),
            ],
            vec!["operation", "tool_solid_id", "target_solid_id"],
        ),
        operation_tool_schema(
            "extrude",
            "Extrude a face or sketch profile along a direction to create a solid",
            vec![
                ("face_id", "ID of the face to extrude", json!({"type": "integer"})),
                ("distance", "Extrusion distance in mm", json!({"type": "number", "exclusiveMinimum": 0})),
                ("direction", "Extrusion direction vector", json!({"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}}, "required": ["x", "y", "z"]})),
            ],
            vec!["face_id", "distance"],
        ),
        operation_tool_schema(
            "revolve",
            "Revolve a face or profile around an axis to create a solid of revolution",
            vec![
                ("face_id", "ID of the face to revolve", json!({"type": "integer"})),
                ("axis_origin", "Point on the revolution axis", json!({"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}}, "required": ["x", "y", "z"]})),
                ("axis_direction", "Direction of the revolution axis", json!({"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}}, "required": ["x", "y", "z"]})),
                ("angle", "Revolution angle in degrees (default 360 for full revolution)", json!({"type": "number", "minimum": 0, "maximum": 360, "default": 360})),
            ],
            vec!["face_id", "axis_origin", "axis_direction"],
        ),
        operation_tool_schema(
            "chamfer_edges",
            "Apply a chamfer to one or more edges of a solid",
            vec![
                ("solid_id", "ID of the solid", json!({"type": "integer"})),
                ("edge_ids", "IDs of edges to chamfer", json!({"type": "array", "items": {"type": "integer"}})),
                ("distance", "Chamfer distance in mm", json!({"type": "number", "exclusiveMinimum": 0})),
            ],
            vec!["solid_id", "edge_ids", "distance"],
        ),
        operation_tool_schema(
            "fillet_edges",
            "Apply a fillet (rounded edge) to one or more edges of a solid",
            vec![
                ("solid_id", "ID of the solid", json!({"type": "integer"})),
                ("edge_ids", "IDs of edges to fillet", json!({"type": "array", "items": {"type": "integer"}})),
                ("radius", "Fillet radius in mm", json!({"type": "number", "exclusiveMinimum": 0})),
            ],
            vec!["solid_id", "edge_ids", "radius"],
        ),
        operation_tool_schema(
            "transform_solid",
            "Apply a transformation (translate, rotate, scale) to a solid",
            vec![
                ("solid_id", "ID of the solid to transform", json!({"type": "integer"})),
                ("translate", "Translation vector [x, y, z] in mm", json!({"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}}})),
                ("rotate_axis", "Rotation axis direction", json!({"type": "object", "properties": {"x": {"type": "number"}, "y": {"type": "number"}, "z": {"type": "number"}}})),
                ("rotate_angle", "Rotation angle in degrees", json!({"type": "number"})),
            ],
            vec!["solid_id"],
        ),
        operation_tool_schema(
            "query_geometry",
            "Get a detailed analysis/summary of a solid's geometry including topology, dimensions, mass properties, and recognized features",
            vec![
                ("solid_id", "ID of the solid to analyze", json!({"type": "integer"})),
            ],
            vec!["solid_id"],
        ),
        operation_tool_schema(
            "export_stl",
            "Export a solid to STL format for 3D printing or visualization",
            vec![
                ("solid_id", "ID of the solid to export", json!({"type": "integer"})),
                ("file_path", "Output file path", json!({"type": "string"})),
                ("binary", "Use binary STL format (smaller, faster)", json!({"type": "boolean", "default": true})),
            ],
            vec!["solid_id", "file_path"],
        ),
    ]
}

/// Generate a tool schema for each registered primitive.
///
/// Since the schema registry may not be fully populated, this function
/// generates schemas from hardcoded primitive definitions that match
/// the actual geometry engine API.
pub fn builtin_primitive_schemas() -> Vec<ToolSchema> {
    vec![
        ToolSchema {
            name: "create_box".into(),
            description: "Create a rectangular box (cuboid) centered at the origin. Specify width (X), height (Y), and depth (Z) dimensions.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "width": { "type": "number", "exclusiveMinimum": 0, "description": "Width along X axis in mm" },
                    "height": { "type": "number", "exclusiveMinimum": 0, "description": "Height along Y axis in mm" },
                    "depth": { "type": "number", "exclusiveMinimum": 0, "description": "Depth along Z axis in mm" },
                },
                "required": ["width", "height", "depth"],
            }),
        },
        ToolSchema {
            name: "create_cylinder".into(),
            description: "Create a cylinder with given radius, height, base center point, and axis direction.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "radius": { "type": "number", "exclusiveMinimum": 0, "description": "Cylinder radius in mm" },
                    "height": { "type": "number", "exclusiveMinimum": 0, "description": "Cylinder height in mm" },
                    "base_center": {
                        "type": "object",
                        "description": "Center of the base circle",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 0},
                    },
                    "axis": {
                        "type": "object",
                        "description": "Cylinder axis direction (default: Z-up)",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 1},
                    },
                },
                "required": ["radius", "height"],
            }),
        },
        ToolSchema {
            name: "create_sphere".into(),
            description: "Create a sphere with given radius and center point.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "radius": { "type": "number", "exclusiveMinimum": 0, "description": "Sphere radius in mm" },
                    "center": {
                        "type": "object",
                        "description": "Center point of the sphere",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 0},
                    },
                },
                "required": ["radius"],
            }),
        },
        ToolSchema {
            name: "create_cone".into(),
            description: "Create a cone or truncated cone (frustum) with base radius, top radius, height, and axis.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "base_radius": { "type": "number", "minimum": 0, "description": "Radius at the base in mm" },
                    "top_radius": { "type": "number", "minimum": 0, "description": "Radius at the top in mm (0 for a point)" },
                    "height": { "type": "number", "exclusiveMinimum": 0, "description": "Cone height in mm" },
                    "base_center": {
                        "type": "object",
                        "description": "Center of the base",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 0},
                    },
                    "axis": {
                        "type": "object",
                        "description": "Cone axis direction",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 1},
                    },
                },
                "required": ["base_radius", "top_radius", "height"],
            }),
        },
        ToolSchema {
            name: "create_torus".into(),
            description: "Create a torus (donut shape) with major radius (center to tube center) and minor radius (tube radius).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "major_radius": { "type": "number", "exclusiveMinimum": 0, "description": "Distance from torus center to tube center in mm" },
                    "minor_radius": { "type": "number", "exclusiveMinimum": 0, "description": "Tube radius in mm" },
                    "center": {
                        "type": "object",
                        "description": "Center point of the torus",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 0},
                    },
                    "axis": {
                        "type": "object",
                        "description": "Torus axis (perpendicular to the ring plane)",
                        "properties": { "x": {"type":"number"}, "y": {"type":"number"}, "z": {"type":"number"} },
                        "required": ["x","y","z"],
                        "default": {"x": 0, "y": 0, "z": 1},
                    },
                },
                "required": ["major_radius", "minor_radius"],
            }),
        },
    ]
}

/// Get all tool schemas for a given tier.
///
/// Returns tool definitions formatted for the Anthropic API `tools` parameter.
pub fn tiered_tool_schemas(tier: ToolTier) -> Vec<ToolSchema> {
    let primitives = builtin_primitive_schemas();
    let operations = builtin_operation_schemas();

    match tier {
        ToolTier::Tier1 => {
            // Core: box, cylinder, sphere + boolean + transform + query
            let core_primitives: Vec<_> = primitives
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.name.as_str(),
                        "create_box" | "create_cylinder" | "create_sphere"
                    )
                })
                .collect();
            let core_ops: Vec<_> = operations
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.name.as_str(),
                        "boolean_operation" | "transform_solid" | "query_geometry" | "export_stl"
                    )
                })
                .collect();
            [core_primitives, core_ops].concat()
        }
        ToolTier::Tier2 => {
            // Tier1 + cone, torus + extrude, revolve, chamfer, fillet
            let tier2_primitives = primitives;
            let tier2_ops: Vec<_> = operations
                .into_iter()
                .filter(|s| {
                    matches!(
                        s.name.as_str(),
                        "boolean_operation"
                            | "transform_solid"
                            | "query_geometry"
                            | "export_stl"
                            | "extrude"
                            | "revolve"
                            | "chamfer_edges"
                            | "fillet_edges"
                    )
                })
                .collect();
            [tier2_primitives, tier2_ops].concat()
        }
        ToolTier::Tier3 => {
            // Everything
            [primitives, operations].concat()
        }
    }
}

/// Convert tool schemas to Anthropic API format.
///
/// Returns a JSON array suitable for the `tools` parameter in the
/// Anthropic Messages API.
pub fn to_anthropic_tools(schemas: &[ToolSchema]) -> Vec<Value> {
    schemas
        .iter()
        .map(|s| {
            json!({
                "name": s.name,
                "description": s.description,
                "input_schema": s.input_schema,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tier1_tool_count() {
        let tools = tiered_tool_schemas(ToolTier::Tier1);
        // Tier1: box + cylinder + sphere + boolean + transform + query + export_stl
        assert!(
            tools.len() >= 5,
            "Tier1 should have at least 5 tools, got {}",
            tools.len()
        );
    }

    #[test]
    fn test_tier3_includes_all() {
        let tier3 = tiered_tool_schemas(ToolTier::Tier3);
        let tier1 = tiered_tool_schemas(ToolTier::Tier1);
        assert!(
            tier3.len() > tier1.len(),
            "Tier3 ({}) should have more tools than Tier1 ({})",
            tier3.len(),
            tier1.len()
        );
    }

    #[test]
    fn test_tool_schema_valid_json() {
        let tools = tiered_tool_schemas(ToolTier::Tier3);
        for tool in &tools {
            // Every tool must have name, description, and valid input_schema
            assert!(!tool.name.is_empty());
            assert!(!tool.description.is_empty());
            assert!(tool.input_schema.is_object());
            assert_eq!(tool.input_schema["type"], "object");
            assert!(tool.input_schema["properties"].is_object());
        }
    }

    #[test]
    fn test_anthropic_format() {
        let tools = tiered_tool_schemas(ToolTier::Tier1);
        let anthropic = to_anthropic_tools(&tools);

        for tool_json in &anthropic {
            assert!(tool_json["name"].is_string());
            assert!(tool_json["description"].is_string());
            assert!(tool_json["input_schema"].is_object());
        }
    }

    #[test]
    fn test_create_box_schema() {
        let schemas = builtin_primitive_schemas();
        let box_schema = schemas.iter().find(|s| s.name == "create_box").unwrap();

        let props = &box_schema.input_schema["properties"];
        assert!(props["width"].is_object());
        assert!(props["height"].is_object());
        assert!(props["depth"].is_object());

        let required = box_schema.input_schema["required"].as_array().unwrap();
        assert!(required.contains(&json!("width")));
        assert!(required.contains(&json!("height")));
        assert!(required.contains(&json!("depth")));
    }

    #[test]
    fn test_boolean_operation_schema() {
        let schemas = builtin_operation_schemas();
        let bool_schema = schemas
            .iter()
            .find(|s| s.name == "boolean_operation")
            .unwrap();

        let props = &bool_schema.input_schema["properties"];
        let op_enum = props["operation"]["enum"].as_array().unwrap();
        assert!(op_enum.contains(&json!("union")));
        assert!(op_enum.contains(&json!("difference")));
        assert!(op_enum.contains(&json!("intersection")));
    }
}
