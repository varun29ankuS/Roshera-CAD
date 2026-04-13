//! Comprehensive Examples for AI Training and Testing
//!
//! This module provides extensive examples for all CAD primitives, designed
//! for AI training, testing, and user guidance. Each example includes multiple
//! input variations, expected outputs, and comprehensive edge case coverage.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Comprehensive example set for a primitive type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrimitiveExampleSet {
    pub primitive_type: String,
    pub basic_examples: Vec<BasicExample>,
    pub edge_case_examples: Vec<EdgeCaseExample>,
    pub error_examples: Vec<ErrorExample>,
    pub advanced_examples: Vec<AdvancedExample>,
    pub integration_examples: Vec<IntegrationExample>,
}

/// Basic usage example with multiple input variations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BasicExample {
    pub name: String,
    pub description: String,
    pub primary_input: String,
    pub input_variations: Vec<InputVariation>,
    pub expected_parameters: HashMap<String, serde_json::Value>,
    pub expected_output: ExpectedOutput,
    pub learning_notes: Vec<String>,
}

/// Input variation for training AI on different phrasings
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InputVariation {
    pub input: String,
    pub style: String, // "formal", "casual", "technical", "abbreviated"
    pub confidence_expected: f64,
    pub parsing_notes: Vec<String>,
}

/// Expected output specification for validation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedOutput {
    pub geometry_type: String,
    pub topology_info: TopologyExpectation,
    pub geometric_properties: GeometricExpectation,
    pub validation_requirements: Vec<ValidationRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TopologyExpectation {
    pub vertex_count: usize,
    pub edge_count: usize,
    pub face_count: usize,
    pub euler_characteristic: i32,
    pub is_manifold: bool,
    pub is_closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeometricExpectation {
    pub volume_formula: String,
    pub surface_area_formula: String,
    pub bounding_box_formula: String,
    pub center_of_mass: [f64; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationRequirement {
    pub check_type: String,
    pub description: String,
    pub tolerance: f64,
}

/// Edge case example for robustness testing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EdgeCaseExample {
    pub name: String,
    pub description: String,
    pub input: String,
    pub edge_case_type: EdgeCaseType,
    pub expected_behavior: String,
    pub validation_strategy: String,
    pub recovery_suggestions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EdgeCaseType {
    MinimumValues,
    MaximumValues,
    ExtremeRatios,
    NumericalPrecision,
    DegenerateGeometry,
    BoundaryConditions,
    UnitConversion,
    ParameterConflicts,
}

/// Error example for error handling training
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorExample {
    pub name: String,
    pub description: String,
    pub input: String,
    pub error_type: String,
    pub expected_error_message: String,
    pub correction_suggestions: Vec<String>,
    pub similar_correct_examples: Vec<String>,
}

/// Advanced usage example for complex scenarios
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdvancedExample {
    pub name: String,
    pub description: String,
    pub input: String,
    pub complexity_factors: Vec<String>,
    pub expected_parameters: HashMap<String, serde_json::Value>,
    pub performance_expectations: PerformanceExpectation,
    pub follow_up_operations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceExpectation {
    pub max_creation_time_ms: f64,
    pub max_memory_usage_mb: f64,
    pub complexity_score: f64,
}

/// Integration example with other operations
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IntegrationExample {
    pub name: String,
    pub description: String,
    pub sequence: Vec<OperationStep>,
    pub final_validation: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationStep {
    pub step_number: usize,
    pub operation: String,
    pub parameters: HashMap<String, serde_json::Value>,
    pub expected_result: String,
}

/// Example generator for all primitives
pub struct ExampleGenerator;

impl ExampleGenerator {
    /// Generate comprehensive examples for box primitive
    pub fn box_examples() -> PrimitiveExampleSet {
        PrimitiveExampleSet {
            primitive_type: "box".to_string(),
            basic_examples: vec![
                Self::basic_box_example(),
                Self::cube_example(),
                Self::rectangular_plate_example(),
                Self::unit_conversion_example(),
            ],
            edge_case_examples: vec![
                Self::minimum_dimension_example(),
                Self::maximum_dimension_example(),
                Self::extreme_ratio_example(),
                Self::precision_limit_example(),
            ],
            error_examples: vec![
                Self::negative_dimension_error(),
                Self::zero_dimension_error(),
                Self::missing_parameter_error(),
                Self::invalid_unit_error(),
            ],
            advanced_examples: vec![
                Self::high_precision_box_example(),
                Self::large_scale_box_example(),
                Self::rounded_corner_box_example(),
            ],
            integration_examples: vec![
                Self::boolean_operation_example(),
                Self::array_pattern_example(),
            ],
        }
    }

    /// Basic box creation with standard dimensions
    fn basic_box_example() -> BasicExample {
        BasicExample {
            name: "Standard Box Creation".to_string(),
            description: "Creates a basic rectangular box with explicit dimensions".to_string(),
            primary_input: "create a box with width 10, height 5, depth 3".to_string(),
            input_variations: vec![
                InputVariation {
                    input: "make a box 10 wide, 5 tall, 3 deep".to_string(),
                    style: "casual".to_string(),
                    confidence_expected: 0.95,
                    parsing_notes: vec![
                        "Uses synonyms: 'tall' for height, 'deep' for depth".to_string()
                    ],
                },
                InputVariation {
                    input: "box: w=10, h=5, d=3".to_string(),
                    style: "abbreviated".to_string(),
                    confidence_expected: 0.92,
                    parsing_notes: vec!["Abbreviated parameter names".to_string()],
                },
                InputVariation {
                    input: "rectangular prism with dimensions 10x5x3".to_string(),
                    style: "technical".to_string(),
                    confidence_expected: 0.88,
                    parsing_notes: vec!["Technical terminology, dimensional notation".to_string()],
                },
                InputVariation {
                    input: "create box (10, 5, 3)".to_string(),
                    style: "mathematical".to_string(),
                    confidence_expected: 0.85,
                    parsing_notes: vec!["Mathematical tuple notation".to_string()],
                },
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(10.0)),
                ("height".to_string(), serde_json::json!(5.0)),
                ("depth".to_string(), serde_json::json!(3.0)),
            ]
            .into(),
            expected_output: ExpectedOutput {
                geometry_type: "box".to_string(),
                topology_info: TopologyExpectation {
                    vertex_count: 8,
                    edge_count: 12,
                    face_count: 6,
                    euler_characteristic: 2,
                    is_manifold: true,
                    is_closed: true,
                },
                geometric_properties: GeometricExpectation {
                    volume_formula: "width * height * depth".to_string(),
                    surface_area_formula: "2 * (width*height + width*depth + height*depth)"
                        .to_string(),
                    bounding_box_formula:
                        "min=(-width/2, -height/2, -depth/2), max=(width/2, height/2, depth/2)"
                            .to_string(),
                    center_of_mass: [0.0, 0.0, 0.0],
                },
                validation_requirements: vec![
                    ValidationRequirement {
                        check_type: "euler_characteristic".to_string(),
                        description: "Must satisfy V - E + F = 2".to_string(),
                        tolerance: 0.0,
                    },
                    ValidationRequirement {
                        check_type: "manifold".to_string(),
                        description: "Must be a closed manifold".to_string(),
                        tolerance: 1e-10,
                    },
                ],
            },
            learning_notes: vec![
                "This is the most common box creation pattern".to_string(),
                "Parameter order doesn't matter in natural language".to_string(),
                "Default units are millimeters".to_string(),
            ],
        }
    }

    /// Cube creation (equal dimensions)
    fn cube_example() -> BasicExample {
        BasicExample {
            name: "Cube Creation".to_string(),
            description: "Creates a cube with equal dimensions".to_string(),
            primary_input: "create a cube with size 5".to_string(),
            input_variations: vec![
                InputVariation {
                    input: "make a 5mm cube".to_string(),
                    style: "concise".to_string(),
                    confidence_expected: 0.93,
                    parsing_notes: vec!["Includes unit specification".to_string()],
                },
                InputVariation {
                    input: "cube side length 5".to_string(),
                    style: "descriptive".to_string(),
                    confidence_expected: 0.91,
                    parsing_notes: vec!["Uses 'side length' terminology".to_string()],
                },
                InputVariation {
                    input: "5x5x5 cube".to_string(),
                    style: "dimensional".to_string(),
                    confidence_expected: 0.89,
                    parsing_notes: vec!["Explicit dimensional notation".to_string()],
                },
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(5.0)),
                ("height".to_string(), serde_json::json!(5.0)),
                ("depth".to_string(), serde_json::json!(5.0)),
            ]
            .into(),
            expected_output: ExpectedOutput {
                geometry_type: "box".to_string(),
                topology_info: TopologyExpectation {
                    vertex_count: 8,
                    edge_count: 12,
                    face_count: 6,
                    euler_characteristic: 2,
                    is_manifold: true,
                    is_closed: true,
                },
                geometric_properties: GeometricExpectation {
                    volume_formula: "size^3".to_string(),
                    surface_area_formula: "6 * size^2".to_string(),
                    bounding_box_formula:
                        "min=(-size/2, -size/2, -size/2), max=(size/2, size/2, size/2)".to_string(),
                    center_of_mass: [0.0, 0.0, 0.0],
                },
                validation_requirements: vec![ValidationRequirement {
                    check_type: "symmetry".to_string(),
                    description: "All dimensions must be equal".to_string(),
                    tolerance: 1e-10,
                }],
            },
            learning_notes: vec![
                "AI should infer equal dimensions from 'cube' keyword".to_string(),
                "'size' parameter applies to all dimensions".to_string(),
            ],
        }
    }

    /// Rectangular plate (thin geometry)
    fn rectangular_plate_example() -> BasicExample {
        BasicExample {
            name: "Thin Plate Creation".to_string(),
            description: "Creates a thin, plate-like geometry".to_string(),
            primary_input: "create a plate 100 wide, 50 deep, 2 thick".to_string(),
            input_variations: vec![
                InputVariation {
                    input: "thin sheet 100x50x2".to_string(),
                    style: "technical".to_string(),
                    confidence_expected: 0.87,
                    parsing_notes: vec!["'sheet' synonym for plate".to_string()],
                },
                InputVariation {
                    input: "flat panel 100mm by 50mm by 2mm".to_string(),
                    style: "explicit_units".to_string(),
                    confidence_expected: 0.92,
                    parsing_notes: vec!["Explicit unit specification".to_string()],
                },
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(100.0)),
                ("height".to_string(), serde_json::json!(2.0)),
                ("depth".to_string(), serde_json::json!(50.0)),
            ]
            .into(),
            expected_output: ExpectedOutput {
                geometry_type: "box".to_string(),
                topology_info: TopologyExpectation {
                    vertex_count: 8,
                    edge_count: 12,
                    face_count: 6,
                    euler_characteristic: 2,
                    is_manifold: true,
                    is_closed: true,
                },
                geometric_properties: GeometricExpectation {
                    volume_formula: "100 * 2 * 50".to_string(),
                    surface_area_formula: "2 * (100*2 + 100*50 + 2*50)".to_string(),
                    bounding_box_formula: "Aspect ratio 50:1:25".to_string(),
                    center_of_mass: [0.0, 0.0, 0.0],
                },
                validation_requirements: vec![ValidationRequirement {
                    check_type: "aspect_ratio".to_string(),
                    description: "Large aspect ratio should be handled correctly".to_string(),
                    tolerance: 1e-6,
                }],
            },
            learning_notes: vec![
                "AI should recognize plate/sheet/panel as box variants".to_string(),
                "'thick' typically refers to height dimension".to_string(),
                "Large aspect ratios require careful numerical handling".to_string(),
            ],
        }
    }

    /// Unit conversion example
    fn unit_conversion_example() -> BasicExample {
        BasicExample {
            name: "Unit Conversion".to_string(),
            description: "Demonstrates automatic unit conversion".to_string(),
            primary_input: "create a box 1 inch wide, 2 cm tall, 10 mm deep".to_string(),
            input_variations: vec![
                InputVariation {
                    input: "box: 1\" x 2cm x 10mm".to_string(),
                    style: "mixed_notation".to_string(),
                    confidence_expected: 0.85,
                    parsing_notes: vec!["Mixed unit symbols".to_string()],
                },
                InputVariation {
                    input: "1 inch by 2 centimeters by 10 millimeters box".to_string(),
                    style: "full_unit_names".to_string(),
                    confidence_expected: 0.88,
                    parsing_notes: vec!["Full unit names".to_string()],
                },
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(25.4)), // 1 inch = 25.4 mm
                ("height".to_string(), serde_json::json!(20.0)), // 2 cm = 20 mm
                ("depth".to_string(), serde_json::json!(10.0)), // 10 mm = 10 mm
            ]
            .into(),
            expected_output: ExpectedOutput {
                geometry_type: "box".to_string(),
                topology_info: TopologyExpectation {
                    vertex_count: 8,
                    edge_count: 12,
                    face_count: 6,
                    euler_characteristic: 2,
                    is_manifold: true,
                    is_closed: true,
                },
                geometric_properties: GeometricExpectation {
                    volume_formula: "25.4 * 20.0 * 10.0".to_string(),
                    surface_area_formula: "2 * (25.4*20.0 + 25.4*10.0 + 20.0*10.0)".to_string(),
                    bounding_box_formula: "All units converted to mm".to_string(),
                    center_of_mass: [0.0, 0.0, 0.0],
                },
                validation_requirements: vec![ValidationRequirement {
                    check_type: "unit_consistency".to_string(),
                    description: "All units must be converted to mm".to_string(),
                    tolerance: 1e-6,
                }],
            },
            learning_notes: vec![
                "AI must handle mixed units correctly".to_string(),
                "All units are normalized to millimeters internally".to_string(),
                "Unit symbols and full names should both work".to_string(),
            ],
        }
    }

    /// Minimum dimension edge case
    fn minimum_dimension_example() -> EdgeCaseExample {
        EdgeCaseExample {
            name: "Minimum Dimension Limit".to_string(),
            description: "Tests behavior at minimum dimension limit".to_string(),
            input: "create a box with width 0.001, height 0.001, depth 0.001".to_string(),
            edge_case_type: EdgeCaseType::MinimumValues,
            expected_behavior: "Should create valid geometry at precision limit".to_string(),
            validation_strategy: "Check numerical stability and topology validity".to_string(),
            recovery_suggestions: vec![
                "Increase dimensions if numerical instability detected".to_string(),
                "Use higher precision arithmetic if needed".to_string(),
            ],
        }
    }

    /// Maximum dimension edge case
    fn maximum_dimension_example() -> EdgeCaseExample {
        EdgeCaseExample {
            name: "Maximum Dimension Limit".to_string(),
            description: "Tests behavior at maximum dimension limit".to_string(),
            input: "create a box with width 1000000, height 1000000, depth 1000000".to_string(),
            edge_case_type: EdgeCaseType::MaximumValues,
            expected_behavior: "Should handle large dimensions without overflow".to_string(),
            validation_strategy: "Check for numerical overflow and memory usage".to_string(),
            recovery_suggestions: vec![
                "Use double precision throughout".to_string(),
                "Check memory allocation limits".to_string(),
            ],
        }
    }

    /// Extreme aspect ratio edge case
    fn extreme_ratio_example() -> EdgeCaseExample {
        EdgeCaseExample {
            name: "Extreme Aspect Ratio".to_string(),
            description: "Tests geometry with extreme aspect ratios".to_string(),
            input: "create a box with width 1000, height 0.001, depth 1000".to_string(),
            edge_case_type: EdgeCaseType::ExtremeRatios,
            expected_behavior: "Should create valid thin geometry".to_string(),
            validation_strategy: "Check topology integrity with extreme ratios".to_string(),
            recovery_suggestions: vec![
                "Verify edge lengths and face areas".to_string(),
                "Check normal vector calculations".to_string(),
            ],
        }
    }

    /// Numerical precision edge case
    fn precision_limit_example() -> EdgeCaseExample {
        EdgeCaseExample {
            name: "Numerical Precision Limit".to_string(),
            description: "Tests behavior at floating-point precision limits".to_string(),
            input: "create a box with width 10.000000000001, height 10.000000000002, depth 10.000000000003".to_string(),
            edge_case_type: EdgeCaseType::NumericalPrecision,
            expected_behavior: "Should handle precision correctly".to_string(),
            validation_strategy: "Verify parameter storage and retrieval accuracy".to_string(),
            recovery_suggestions: vec![
                "Use tolerance-based comparisons".to_string(),
                "Document precision limitations".to_string(),
            ],
        }
    }

    /// Negative dimension error
    fn negative_dimension_error() -> ErrorExample {
        ErrorExample {
            name: "Negative Dimension".to_string(),
            description: "Tests error handling for negative dimensions".to_string(),
            input: "create a box with width -5, height 10, depth 3".to_string(),
            error_type: "InvalidParameters".to_string(),
            expected_error_message: "Invalid parameter 'width' = '-5': must be positive"
                .to_string(),
            correction_suggestions: vec![
                "Use positive values for all dimensions".to_string(),
                "Example: 'create a box with width 5, height 10, depth 3'".to_string(),
            ],
            similar_correct_examples: vec![
                "create a box with width 5, height 10, depth 3".to_string(),
                "make a box 5 wide, 10 tall, 3 deep".to_string(),
            ],
        }
    }

    /// Zero dimension error
    fn zero_dimension_error() -> ErrorExample {
        ErrorExample {
            name: "Zero Dimension".to_string(),
            description: "Tests error handling for zero dimensions".to_string(),
            input: "create a box with width 0, height 5, depth 3".to_string(),
            error_type: "InvalidParameters".to_string(),
            expected_error_message: "Invalid parameter 'width' = '0': must be positive".to_string(),
            correction_suggestions: vec![
                "All dimensions must be greater than zero".to_string(),
                "Minimum dimension is 0.001".to_string(),
            ],
            similar_correct_examples: vec![
                "create a box with width 0.1, height 5, depth 3".to_string()
            ],
        }
    }

    /// Missing parameter error
    fn missing_parameter_error() -> ErrorExample {
        ErrorExample {
            name: "Missing Required Parameter".to_string(),
            description: "Tests error handling for missing parameters".to_string(),
            input: "create a box with width 10, height 5".to_string(),
            error_type: "MissingParameter".to_string(),
            expected_error_message: "Missing required parameter: depth".to_string(),
            correction_suggestions: vec![
                "Specify all three dimensions: width, height, depth".to_string(),
                "Example: 'create a box with width 10, height 5, depth 3'".to_string(),
            ],
            similar_correct_examples: vec![
                "create a box with width 10, height 5, depth 3".to_string()
            ],
        }
    }

    /// Invalid unit error
    fn invalid_unit_error() -> ErrorExample {
        ErrorExample {
            name: "Invalid Unit".to_string(),
            description: "Tests error handling for unsupported units".to_string(),
            input: "create a box with width 10 kilometers, height 5, depth 3".to_string(),
            error_type: "InvalidUnit".to_string(),
            expected_error_message:
                "Unsupported unit 'kilometers'. Supported units: mm, cm, m, in, ft".to_string(),
            correction_suggestions: vec![
                "Use supported units: mm, cm, m, in, ft".to_string(),
                "Convert to meters: '10 kilometers' = '10000 m'".to_string(),
            ],
            similar_correct_examples: vec![
                "create a box with width 10000 m, height 5, depth 3".to_string()
            ],
        }
    }

    /// High precision box example
    fn high_precision_box_example() -> AdvancedExample {
        AdvancedExample {
            name: "High Precision Box".to_string(),
            description: "Creates a box with high-precision dimensions".to_string(),
            input: "create a precision box with width 10.123456789, height 5.987654321, depth 3.555555555".to_string(),
            complexity_factors: vec![
                "High decimal precision".to_string(),
                "Floating-point accuracy requirements".to_string(),
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(10.123456789)),
                ("height".to_string(), serde_json::json!(5.987654321)),
                ("depth".to_string(), serde_json::json!(3.555555555)),
            ].into(),
            performance_expectations: PerformanceExpectation {
                max_creation_time_ms: 2.0,
                max_memory_usage_mb: 1.0,
                complexity_score: 1.5,
            },
            follow_up_operations: vec![
                "Validate precision retention".to_string(),
                "Test geometric calculations".to_string(),
            ],
        }
    }

    /// Large scale box example
    fn large_scale_box_example() -> AdvancedExample {
        AdvancedExample {
            name: "Large Scale Box".to_string(),
            description: "Creates a box with very large dimensions".to_string(),
            input: "create a building-scale box with width 100 meters, height 50 meters, depth 200 meters".to_string(),
            complexity_factors: vec![
                "Large scale geometry".to_string(),
                "Unit conversion from meters".to_string(),
                "Memory usage considerations".to_string(),
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(100000.0)), // 100m = 100,000mm
                ("height".to_string(), serde_json::json!(50000.0)),  // 50m = 50,000mm
                ("depth".to_string(), serde_json::json!(200000.0)), // 200m = 200,000mm
            ].into(),
            performance_expectations: PerformanceExpectation {
                max_creation_time_ms: 5.0,
                max_memory_usage_mb: 2.0,
                complexity_score: 2.0,
            },
            follow_up_operations: vec![
                "Check numerical stability".to_string(),
                "Validate large-scale calculations".to_string(),
            ],
        }
    }

    /// Rounded corner box example
    fn rounded_corner_box_example() -> AdvancedExample {
        AdvancedExample {
            name: "Rounded Corner Box".to_string(),
            description: "Creates a box with rounded corners".to_string(),
            input: "create a rounded box 20x15x10 with corner radius 2".to_string(),
            complexity_factors: vec![
                "Optional parameter usage".to_string(),
                "Corner radius validation".to_string(),
                "Complex topology generation".to_string(),
            ],
            expected_parameters: [
                ("width".to_string(), serde_json::json!(20.0)),
                ("height".to_string(), serde_json::json!(15.0)),
                ("depth".to_string(), serde_json::json!(10.0)),
                ("corner_radius".to_string(), serde_json::json!(2.0)),
            ]
            .into(),
            performance_expectations: PerformanceExpectation {
                max_creation_time_ms: 10.0,
                max_memory_usage_mb: 3.0,
                complexity_score: 3.0,
            },
            follow_up_operations: vec![
                "Validate corner radius constraints".to_string(),
                "Check surface continuity".to_string(),
            ],
        }
    }

    /// Boolean operation integration example
    fn boolean_operation_example() -> IntegrationExample {
        IntegrationExample {
            name: "Boolean Union with Box".to_string(),
            description: "Creates two boxes and performs boolean union".to_string(),
            sequence: vec![
                OperationStep {
                    step_number: 1,
                    operation: "create_box".to_string(),
                    parameters: [
                        ("width".to_string(), serde_json::json!(10.0)),
                        ("height".to_string(), serde_json::json!(10.0)),
                        ("depth".to_string(), serde_json::json!(10.0)),
                    ]
                    .into(),
                    expected_result: "Box A created at origin".to_string(),
                },
                OperationStep {
                    step_number: 2,
                    operation: "create_box".to_string(),
                    parameters: [
                        ("width".to_string(), serde_json::json!(10.0)),
                        ("height".to_string(), serde_json::json!(10.0)),
                        ("depth".to_string(), serde_json::json!(10.0)),
                        ("offset".to_string(), serde_json::json!([5.0, 0.0, 0.0])),
                    ]
                    .into(),
                    expected_result: "Box B created offset by 5mm in X".to_string(),
                },
                OperationStep {
                    step_number: 3,
                    operation: "boolean_union".to_string(),
                    parameters: [
                        ("object_a".to_string(), serde_json::json!("box_a")),
                        ("object_b".to_string(), serde_json::json!("box_b")),
                    ]
                    .into(),
                    expected_result: "Combined geometry with overlapping volume merged".to_string(),
                },
            ],
            final_validation: "Result should be a single manifold solid".to_string(),
        }
    }

    /// Array pattern integration example
    fn array_pattern_example() -> IntegrationExample {
        IntegrationExample {
            name: "Box Array Pattern".to_string(),
            description: "Creates an array of boxes in a pattern".to_string(),
            sequence: vec![
                OperationStep {
                    step_number: 1,
                    operation: "create_box".to_string(),
                    parameters: [
                        ("width".to_string(), serde_json::json!(5.0)),
                        ("height".to_string(), serde_json::json!(5.0)),
                        ("depth".to_string(), serde_json::json!(5.0)),
                    ]
                    .into(),
                    expected_result: "Base cube created".to_string(),
                },
                OperationStep {
                    step_number: 2,
                    operation: "linear_array".to_string(),
                    parameters: [
                        ("object".to_string(), serde_json::json!("base_cube")),
                        ("direction".to_string(), serde_json::json!([1.0, 0.0, 0.0])),
                        ("spacing".to_string(), serde_json::json!(10.0)),
                        ("count".to_string(), serde_json::json!(5)),
                    ]
                    .into(),
                    expected_result: "5 cubes in a row along X-axis".to_string(),
                },
            ],
            final_validation: "Array should contain 5 separate cube instances".to_string(),
        }
    }
}
