//! Natural Language Schemas for AI-First CAD Interface
//!
//! This module provides human-readable, AI-friendly descriptions of all CAD
//! primitives and their parameters. AI systems can use these schemas to:
//!
//! 1. **Understand Intent**: Map natural language to technical parameters
//! 2. **Generate Examples**: Create training data for AI models
//! 3. **Provide Help**: Offer contextual assistance to users
//! 4. **Validate Input**: Check parameter ranges and constraints
//! 5. **Auto-Complete**: Suggest parameter values and alternatives

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Comprehensive schema for a primitive with AI-friendly metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimitiveSchema {
    /// Technical identifier
    pub id: String,
    /// Human-readable name
    pub display_name: String,
    /// Natural language description
    pub description: String,
    /// Extended explanation for AI understanding
    pub ai_explanation: String,
    /// Alternative names and synonyms
    pub aliases: Vec<String>,
    /// Categories for organization
    pub categories: Vec<String>,
    /// Parameter information
    pub parameters: Vec<ParameterInfo>,
    /// Usage examples
    pub examples: Vec<CommandExample>,
    /// Common use cases
    pub use_cases: Vec<UseCase>,
    /// Visual description for AI visualization
    pub visual_description: String,
    /// Complexity rating (1-5, where 1 is simplest)
    pub complexity: u8,
    /// Typical creation time in milliseconds
    pub typical_time_ms: f64,
}

/// Detailed parameter information for AI understanding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterInfo {
    /// Technical parameter name
    pub name: String,
    /// Human-readable name
    pub display_name: String,
    /// Natural language description
    pub description: String,
    /// What this parameter controls
    pub purpose: String,
    /// Parameter type information
    pub data_type: ParameterDataType,
    /// Whether this parameter is required
    pub required: bool,
    /// Default value if not specified
    pub default_value: Option<serde_json::Value>,
    /// Valid range and constraints
    pub constraints: ParameterConstraints,
    /// Common values and examples
    pub common_values: Vec<CommonValue>,
    /// Alternative names for this parameter
    pub aliases: Vec<String>,
    /// Units information
    pub units: UnitInfo,
    /// How this parameter affects the geometry
    pub effect_description: String,
    /// Visual impact description
    pub visual_impact: String,
}

/// Parameter data type with AI-friendly metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ParameterDataType {
    Number {
        precision: u8,
        format_hint: String, // e.g., "integer", "decimal", "percentage"
    },
    Length {
        default_unit: String,
        precision: u8,
    },
    Angle {
        default_unit: String, // "degrees" or "radians"
        precision: u8,
    },
    Boolean {
        true_synonyms: Vec<String>,  // "yes", "on", "true", "enabled"
        false_synonyms: Vec<String>, // "no", "off", "false", "disabled"
    },
    Choice {
        options: Vec<ChoiceOption>,
    },
    Point3D {
        coordinate_system: String, // "cartesian", "cylindrical", "spherical"
    },
    Vector3D {
        normalized: bool,
    },
    Text {
        max_length: Option<usize>,
        pattern: Option<String>,
    },
}

/// Choice option with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChoiceOption {
    pub value: String,
    pub display_name: String,
    pub description: String,
    pub aliases: Vec<String>,
}

/// Parameter constraints for validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterConstraints {
    /// Minimum value (for numbers)
    pub min: Option<f64>,
    /// Maximum value (for numbers)
    pub max: Option<f64>,
    /// Suggested minimum for typical use
    pub typical_min: Option<f64>,
    /// Suggested maximum for typical use
    pub typical_max: Option<f64>,
    /// Must be positive
    pub positive_only: bool,
    /// Must be integer
    pub integer_only: bool,
    /// Custom validation rules
    pub custom_rules: Vec<ValidationRule>,
    /// Dependencies on other parameters
    pub dependencies: Vec<ParameterDependency>,
}

/// Custom validation rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRule {
    pub rule_type: String,
    pub description: String,
    pub expression: String, // Mathematical expression or rule
    pub error_message: String,
}

/// Parameter dependency relationship
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDependency {
    pub depends_on: String,
    pub relationship: DependencyType,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum DependencyType {
    MustBeLessThan,
    MustBeGreaterThan,
    MustEqual,
    MustNotEqual,
    ConditionalRequired { condition: String },
    Custom { expression: String },
}

/// Common parameter values with context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommonValue {
    pub value: serde_json::Value,
    pub description: String,
    pub use_case: String,
    pub frequency: f64, // How often this value is used (0.0-1.0)
}

/// Unit information for AI understanding
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitInfo {
    /// Primary unit (e.g., "mm")
    pub primary: String,
    /// Supported units with conversion factors
    pub supported: HashMap<String, UnitDetails>,
    /// Whether unit is required in input
    pub unit_required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnitDetails {
    pub name: String,
    pub symbol: String,
    pub conversion_factor: f64, // Factor to convert to primary unit
    pub common_usage: String,
    pub aliases: Vec<String>,
}

/// Usage example with comprehensive metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandExample {
    /// Natural language input
    pub input: String,
    /// What this example demonstrates
    pub purpose: String,
    /// Expected parameter extraction
    pub parameters: HashMap<String, serde_json::Value>,
    /// Human description of the result
    pub result_description: String,
    /// Tags for categorization
    pub tags: Vec<String>,
    /// Difficulty level (1-5)
    pub difficulty: u8,
    /// Variations of this example
    pub variations: Vec<ExampleVariation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExampleVariation {
    pub input: String,
    pub description: String,
    pub difference: String, // What's different from the main example
}

/// Use case description for AI context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UseCase {
    pub title: String,
    pub description: String,
    pub domain: String, // "mechanical", "architectural", "artistic", etc.
    pub complexity: u8,
    pub typical_parameters: HashMap<String, serde_json::Value>,
    pub related_operations: Vec<String>,
}

/// Built-in schema generator for all primitives
pub struct SchemaGenerator;

impl SchemaGenerator {
    /// Generate comprehensive schema for box primitive
    pub fn box_schema() -> PrimitiveSchema {
        PrimitiveSchema {
            id: "box".to_string(),
            display_name: "Rectangular Box".to_string(),
            description: "Creates a three-dimensional rectangular box (cuboid) with specified dimensions".to_string(),
            ai_explanation: "A box is the most fundamental 3D shape in CAD. It has 8 vertices (corners), 12 edges, and 6 faces. Each face is a rectangle. The box is defined by three perpendicular dimensions: width (X-axis), height (Y-axis), and depth (Z-axis). The box is centered at the origin unless transformed.".to_string(),
            aliases: vec![
                "cube".to_string(),
                "cuboid".to_string(), 
                "rectangular_prism".to_string(),
                "block".to_string(),
                "brick".to_string(),
            ],
            categories: vec![
                "basic_shapes".to_string(),
                "primitives".to_string(),
                "3d_shapes".to_string(),
                "rectangular".to_string(),
            ],
            parameters: vec![
                Self::width_parameter(),
                Self::height_parameter(),
                Self::depth_parameter(),
                Self::corner_radius_parameter(),
            ],
            examples: vec![
                Self::basic_box_example(),
                Self::cube_example(),
                Self::thin_plate_example(),
                Self::rounded_box_example(),
            ],
            use_cases: vec![
                UseCase {
                    title: "Mechanical Enclosures".to_string(),
                    description: "Electronic device housings, control boxes".to_string(),
                    domain: "mechanical".to_string(),
                    complexity: 2,
                    typical_parameters: [
                        ("width".to_string(), serde_json::json!(100)),
                        ("height".to_string(), serde_json::json!(60)),
                        ("depth".to_string(), serde_json::json!(40)),
                    ].into(),
                    related_operations: vec!["fillet".to_string(), "shell".to_string()],
                },
                UseCase {
                    title: "Building Blocks".to_string(),
                    description: "Foundation elements for complex assemblies".to_string(),
                    domain: "architectural".to_string(),
                    complexity: 1,
                    typical_parameters: [
                        ("width".to_string(), serde_json::json!(1000)),
                        ("height".to_string(), serde_json::json!(200)),
                        ("depth".to_string(), serde_json::json!(500)),
                    ].into(),
                    related_operations: vec!["boolean_union".to_string(), "array".to_string()],
                },
            ],
            visual_description: "A three-dimensional rectangular solid with flat faces and sharp edges. Appears as a traditional box or brick shape.".to_string(),
            complexity: 1,
            typical_time_ms: 0.5,
        }
    }

    /// Width parameter definition
    fn width_parameter() -> ParameterInfo {
        ParameterInfo {
            name: "width".to_string(),
            display_name: "Width".to_string(),
            description: "The dimension of the box along the X-axis".to_string(),
            purpose: "Controls how wide the box is from left to right".to_string(),
            data_type: ParameterDataType::Length {
                default_unit: "mm".to_string(),
                precision: 3,
            },
            required: true,
            default_value: Some(serde_json::json!(10.0)),
            constraints: ParameterConstraints {
                min: Some(0.001),
                max: Some(1000000.0),
                typical_min: Some(1.0),
                typical_max: Some(1000.0),
                positive_only: true,
                integer_only: false,
                custom_rules: vec![],
                dependencies: vec![],
            },
            common_values: vec![
                CommonValue {
                    value: serde_json::json!(10.0),
                    description: "Small part width".to_string(),
                    use_case: "Small mechanical components".to_string(),
                    frequency: 0.3,
                },
                CommonValue {
                    value: serde_json::json!(100.0),
                    description: "Medium enclosure width".to_string(),
                    use_case: "Electronic enclosures".to_string(),
                    frequency: 0.4,
                },
                CommonValue {
                    value: serde_json::json!(1000.0),
                    description: "Large structural width".to_string(),
                    use_case: "Building elements".to_string(),
                    frequency: 0.2,
                },
            ],
            aliases: vec![
                "w".to_string(),
                "x".to_string(),
                "x_dimension".to_string(),
                "horizontal".to_string(),
            ],
            units: UnitInfo {
                primary: "mm".to_string(),
                supported: [
                    (
                        "mm".to_string(),
                        UnitDetails {
                            name: "millimeter".to_string(),
                            symbol: "mm".to_string(),
                            conversion_factor: 1.0,
                            common_usage: "Precision parts, small components".to_string(),
                            aliases: vec!["millimeter".to_string(), "millimeters".to_string()],
                        },
                    ),
                    (
                        "cm".to_string(),
                        UnitDetails {
                            name: "centimeter".to_string(),
                            symbol: "cm".to_string(),
                            conversion_factor: 10.0,
                            common_usage: "General modeling".to_string(),
                            aliases: vec!["centimeter".to_string(), "centimeters".to_string()],
                        },
                    ),
                    (
                        "m".to_string(),
                        UnitDetails {
                            name: "meter".to_string(),
                            symbol: "m".to_string(),
                            conversion_factor: 1000.0,
                            common_usage: "Architecture, large structures".to_string(),
                            aliases: vec!["meter".to_string(), "meters".to_string()],
                        },
                    ),
                    (
                        "in".to_string(),
                        UnitDetails {
                            name: "inch".to_string(),
                            symbol: "in".to_string(),
                            conversion_factor: 25.4,
                            common_usage: "Imperial measurements".to_string(),
                            aliases: vec![
                                "inch".to_string(),
                                "inches".to_string(),
                                "\"".to_string(),
                            ],
                        },
                    ),
                ]
                .into(),
                unit_required: false,
            },
            effect_description: "Increasing width makes the box wider along the X-axis".to_string(),
            visual_impact: "The box stretches horizontally (left-right)".to_string(),
        }
    }

    /// Height parameter definition  
    fn height_parameter() -> ParameterInfo {
        ParameterInfo {
            name: "height".to_string(),
            display_name: "Height".to_string(),
            description: "The dimension of the box along the Y-axis".to_string(),
            purpose: "Controls how tall the box is from bottom to top".to_string(),
            data_type: ParameterDataType::Length {
                default_unit: "mm".to_string(),
                precision: 3,
            },
            required: true,
            default_value: Some(serde_json::json!(10.0)),
            constraints: ParameterConstraints {
                min: Some(0.001),
                max: Some(1000000.0),
                typical_min: Some(1.0),
                typical_max: Some(1000.0),
                positive_only: true,
                integer_only: false,
                custom_rules: vec![],
                dependencies: vec![],
            },
            common_values: vec![
                CommonValue {
                    value: serde_json::json!(5.0),
                    description: "Thin profile".to_string(),
                    use_case: "Plates, thin walls".to_string(),
                    frequency: 0.2,
                },
                CommonValue {
                    value: serde_json::json!(10.0),
                    description: "Standard height".to_string(),
                    use_case: "General purpose".to_string(),
                    frequency: 0.4,
                },
                CommonValue {
                    value: serde_json::json!(50.0),
                    description: "Tall component".to_string(),
                    use_case: "Towers, columns".to_string(),
                    frequency: 0.3,
                },
            ],
            aliases: vec![
                "h".to_string(),
                "y".to_string(),
                "y_dimension".to_string(),
                "vertical".to_string(),
                "tall".to_string(),
            ],
            units: UnitInfo {
                primary: "mm".to_string(),
                supported: [
                    (
                        "mm".to_string(),
                        UnitDetails {
                            name: "millimeter".to_string(),
                            symbol: "mm".to_string(),
                            conversion_factor: 1.0,
                            common_usage: "Precision parts".to_string(),
                            aliases: vec!["millimeter".to_string()],
                        },
                    ),
                    (
                        "cm".to_string(),
                        UnitDetails {
                            name: "centimeter".to_string(),
                            symbol: "cm".to_string(),
                            conversion_factor: 10.0,
                            common_usage: "General modeling".to_string(),
                            aliases: vec!["centimeter".to_string()],
                        },
                    ),
                ]
                .into(),
                unit_required: false,
            },
            effect_description: "Increasing height makes the box taller along the Y-axis"
                .to_string(),
            visual_impact: "The box stretches vertically (up-down)".to_string(),
        }
    }

    /// Depth parameter definition
    fn depth_parameter() -> ParameterInfo {
        ParameterInfo {
            name: "depth".to_string(),
            display_name: "Depth".to_string(),
            description: "The dimension of the box along the Z-axis".to_string(),
            purpose: "Controls how deep the box is from front to back".to_string(),
            data_type: ParameterDataType::Length {
                default_unit: "mm".to_string(),
                precision: 3,
            },
            required: true,
            default_value: Some(serde_json::json!(10.0)),
            constraints: ParameterConstraints {
                min: Some(0.001),
                max: Some(1000000.0),
                typical_min: Some(1.0),
                typical_max: Some(1000.0),
                positive_only: true,
                integer_only: false,
                custom_rules: vec![],
                dependencies: vec![],
            },
            common_values: vec![
                CommonValue {
                    value: serde_json::json!(3.0),
                    description: "Thin depth".to_string(),
                    use_case: "Sheets, panels".to_string(),
                    frequency: 0.2,
                },
                CommonValue {
                    value: serde_json::json!(20.0),
                    description: "Standard depth".to_string(),
                    use_case: "General components".to_string(),
                    frequency: 0.5,
                },
            ],
            aliases: vec![
                "d".to_string(),
                "z".to_string(),
                "z_dimension".to_string(),
                "thickness".to_string(),
                "front_to_back".to_string(),
            ],
            units: UnitInfo {
                primary: "mm".to_string(),
                supported: HashMap::new(), // Inherit from width
                unit_required: false,
            },
            effect_description: "Increasing depth makes the box deeper along the Z-axis"
                .to_string(),
            visual_impact: "The box stretches in depth (front-back)".to_string(),
        }
    }

    /// Corner radius parameter (optional)
    fn corner_radius_parameter() -> ParameterInfo {
        ParameterInfo {
            name: "corner_radius".to_string(),
            display_name: "Corner Radius".to_string(),
            description: "Optional radius for rounding the box corners".to_string(),
            purpose: "Creates rounded corners instead of sharp edges".to_string(),
            data_type: ParameterDataType::Length {
                default_unit: "mm".to_string(),
                precision: 3,
            },
            required: false,
            default_value: None,
            constraints: ParameterConstraints {
                min: Some(0.0),
                max: None, // Will be validated against box dimensions
                typical_min: Some(0.5),
                typical_max: Some(10.0),
                positive_only: false, // 0 is allowed for sharp corners
                integer_only: false,
                custom_rules: vec![ValidationRule {
                    rule_type: "max_constraint".to_string(),
                    description: "Corner radius must not exceed half the smallest dimension"
                        .to_string(),
                    expression: "corner_radius <= min(width, height, depth) / 2".to_string(),
                    error_message: "Corner radius too large for box dimensions".to_string(),
                }],
                dependencies: vec![ParameterDependency {
                    depends_on: "width".to_string(),
                    relationship: DependencyType::MustBeLessThan,
                    description: "Corner radius must be less than half the width".to_string(),
                }],
            },
            common_values: vec![
                CommonValue {
                    value: serde_json::json!(1.0),
                    description: "Slight rounding".to_string(),
                    use_case: "Safety edges".to_string(),
                    frequency: 0.6,
                },
                CommonValue {
                    value: serde_json::json!(5.0),
                    description: "Noticeable rounding".to_string(),
                    use_case: "Aesthetic design".to_string(),
                    frequency: 0.3,
                },
            ],
            aliases: vec![
                "radius".to_string(),
                "rounding".to_string(),
                "fillet".to_string(),
                "corner_fillet".to_string(),
            ],
            units: UnitInfo {
                primary: "mm".to_string(),
                supported: HashMap::new(),
                unit_required: false,
            },
            effect_description: "Rounds all corners of the box with the specified radius"
                .to_string(),
            visual_impact: "Sharp corners become smooth, rounded transitions".to_string(),
        }
    }

    /// Basic box creation example
    fn basic_box_example() -> CommandExample {
        CommandExample {
            input: "create a box with width 10, height 5, depth 3".to_string(),
            purpose: "Basic box creation with explicit dimensions".to_string(),
            parameters: [
                ("width".to_string(), serde_json::json!(10.0)),
                ("height".to_string(), serde_json::json!(5.0)),
                ("depth".to_string(), serde_json::json!(3.0)),
            ]
            .into(),
            result_description: "Creates a 10×5×3 mm rectangular box centered at origin"
                .to_string(),
            tags: vec!["basic".to_string(), "explicit_dimensions".to_string()],
            difficulty: 1,
            variations: vec![
                ExampleVariation {
                    input: "make a box 10 by 5 by 3".to_string(),
                    description: "Alternative phrasing with 'by'".to_string(),
                    difference: "Uses 'by' instead of explicit parameter names".to_string(),
                },
                ExampleVariation {
                    input: "box: w=10, h=5, d=3".to_string(),
                    description: "Compact notation".to_string(),
                    difference: "Uses abbreviated parameter names".to_string(),
                },
            ],
        }
    }

    /// Cube creation example  
    fn cube_example() -> CommandExample {
        CommandExample {
            input: "create a cube with size 5".to_string(),
            purpose: "Demonstrates cube creation (equal dimensions)".to_string(),
            parameters: [
                ("width".to_string(), serde_json::json!(5.0)),
                ("height".to_string(), serde_json::json!(5.0)),
                ("depth".to_string(), serde_json::json!(5.0)),
            ]
            .into(),
            result_description: "Creates a 5×5×5 mm cube".to_string(),
            tags: vec!["cube".to_string(), "equal_dimensions".to_string()],
            difficulty: 1,
            variations: vec![ExampleVariation {
                input: "make a 5mm cube".to_string(),
                description: "More concise phrasing".to_string(),
                difference: "Includes unit and more direct language".to_string(),
            }],
        }
    }

    /// Thin plate example
    fn thin_plate_example() -> CommandExample {
        CommandExample {
            input: "create a thin plate 100 wide, 50 deep, 2 thick".to_string(),
            purpose: "Shows how to create thin, plate-like geometry".to_string(),
            parameters: [
                ("width".to_string(), serde_json::json!(100.0)),
                ("height".to_string(), serde_json::json!(2.0)),
                ("depth".to_string(), serde_json::json!(50.0)),
            ]
            .into(),
            result_description: "Creates a 100×2×50 mm thin plate".to_string(),
            tags: vec![
                "plate".to_string(),
                "thin".to_string(),
                "alternative_words".to_string(),
            ],
            difficulty: 2,
            variations: vec![],
        }
    }

    /// Rounded box example
    fn rounded_box_example() -> CommandExample {
        CommandExample {
            input: "create a rounded box 20x15x10 with corner radius 2".to_string(),
            purpose: "Demonstrates optional corner rounding parameter".to_string(),
            parameters: [
                ("width".to_string(), serde_json::json!(20.0)),
                ("height".to_string(), serde_json::json!(15.0)),
                ("depth".to_string(), serde_json::json!(10.0)),
                ("corner_radius".to_string(), serde_json::json!(2.0)),
            ]
            .into(),
            result_description: "Creates a 20×15×10 mm box with 2mm rounded corners".to_string(),
            tags: vec!["rounded".to_string(), "optional_parameter".to_string()],
            difficulty: 3,
            variations: vec![ExampleVariation {
                input: "box 20x15x10 with 2mm fillets".to_string(),
                description: "Uses 'fillets' instead of 'corner radius'".to_string(),
                difference: "Alternative terminology for rounding".to_string(),
            }],
        }
    }

    /// Generate sphere schema (placeholder for future implementation)
    pub fn sphere_schema() -> PrimitiveSchema {
        PrimitiveSchema {
            id: "sphere".to_string(),
            display_name: "Sphere".to_string(),
            description: "Creates a perfect three-dimensional sphere".to_string(),
            ai_explanation: "A sphere is a perfectly round 3D shape where every point on the surface is equidistant from the center.".to_string(),
            aliases: vec!["ball".to_string(), "orb".to_string()],
            categories: vec!["basic_shapes".to_string(), "curved".to_string()],
            parameters: vec![], // TODO: Implement sphere parameters
            examples: vec![],
            use_cases: vec![],
            visual_description: "A perfectly round ball shape".to_string(),
            complexity: 2,
            typical_time_ms: 1.0,
        }
    }
}

// #[cfg(test)]
// mod tests {
//     use super::*;
//
//     #[test]
//     fn test_box_schema_generation() {
//         let schema = SchemaGenerator::box_schema();
//
//         assert_eq!(schema.id, "box");
//         assert_eq!(schema.parameters.len(), 4); // width, height, depth, corner_radius
//         assert!(!schema.examples.is_empty());
//         assert!(!schema.use_cases.is_empty());
//     }
//
//     #[test]
//     fn test_parameter_validation() {
//         let schema = SchemaGenerator::box_schema();
//         let width_param = &schema.parameters[0];
//
//         assert_eq!(width_param.name, "width");
//         assert!(width_param.required);
//         assert!(width_param.constraints.positive_only);
//         assert!(width_param.constraints.min.unwrap() > 0.0);
//     }
//
//     #[test]
//     fn test_unit_conversion_info() {
//         let schema = SchemaGenerator::box_schema();
//         let width_param = &schema.parameters[0];
//
//         assert_eq!(width_param.units.primary, "mm");
//         assert!(width_param.units.supported.contains_key("cm"));
//         assert!(width_param.units.supported.contains_key("in"));
//     }
//
//     #[test]
//     fn test_example_variations() {
//         let schema = SchemaGenerator::box_schema();
//         let basic_example = &schema.examples[0];
//
//         assert!(!basic_example.variations.is_empty());
//         assert!(basic_example.variations[0].input.contains("by"));
//     }
// }
