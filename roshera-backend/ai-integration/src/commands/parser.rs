//! Natural language command parsing with advanced NLP
//!
//! Provides sophisticated pattern matching and context awareness.

use shared_types::*;
use regex::Regex;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use crate::commands::{CommandError, CommandParseResult};

/// Lazy static patterns initialized with error handling
struct Patterns {
    number_regex: Regex,
    color_regex: Regex,
}

static PATTERNS: Lazy<Result<Patterns, regex::Error>> = Lazy::new(|| {
    Ok(Patterns {
        number_regex: Regex::new(r"(\d+(?:\.\d+)?)\s*(mm|cm|m|in|ft)?")?,
        color_regex: Regex::new(r"\b(red|green|blue|yellow|orange|purple|black|white|gray|grey|steel|aluminum|plastic|glass)\b")?,
    })
});

/// Natural language parser with pattern matching
#[derive(Clone)]
pub struct NaturalLanguageParser {
    /// Shape creation patterns
    shape_patterns: Vec<ShapePattern>,
    /// Boolean operation patterns
    boolean_patterns: Vec<BooleanPattern>,
    /// Transform patterns
    transform_patterns: Vec<TransformPattern>,
    /// Context analyzer
    context_analyzer: ContextAnalyzer,
}

/// Pattern for shape creation
#[derive(Clone)]
struct ShapePattern {
    /// Regex pattern
    pattern: Regex,
    /// Shape type
    shape_type: PrimitiveType,
    /// Parameter extractors
    parameter_extractors: Vec<ParameterExtractor>,
}

/// Pattern for boolean operations
#[derive(Clone)]
struct BooleanPattern {
    /// Regex pattern
    pattern: Regex,
    /// Operation type
    operation: BooleanOp,
    /// Keywords that trigger this pattern
    keywords: Vec<String>,
}

/// Pattern for transformations
#[derive(Clone)]
struct TransformPattern {
    /// Regex pattern
    pattern: Regex,
    /// Transform type
    transform_type: TransformPatternType,
}

#[derive(Clone)]
enum TransformPatternType {
    Move,
    Rotate,
    Scale,
    Mirror,
}

/// Parameter extraction configuration
#[derive(Clone)]
struct ParameterExtractor {
    /// Parameter name
    name: String,
    /// Extraction pattern
    pattern: Regex,
    /// Default value
    default_value: f64,
    /// Unit conversion
    unit_type: UnitType,
}

#[derive(Clone)]
enum UnitType {
    Length,
    Angle,
    Count,
    Ratio,
}

/// Context analyzer for smarter parsing
#[derive(Clone)]
pub struct ContextAnalyzer {
    /// Known object names/references
    _object_references: HashMap<String, ObjectId>,
    /// Common abbreviations
    _abbreviations: HashMap<String, String>,
    /// Unit preferences
    _default_units: Units,
}

impl NaturalLanguageParser {
    /// Create new parser
    pub fn new() -> CommandParseResult<Self> {
        let mut parser = Self {
            shape_patterns: Vec::new(),
            boolean_patterns: Vec::new(),
            transform_patterns: Vec::new(),
            context_analyzer: ContextAnalyzer::new(),
        };
        
        parser.initialize_patterns()?;
        Ok(parser)
    }
    
    /// Initialize all patterns
    fn initialize_patterns(&mut self) -> CommandParseResult<()> {
        // Box/Cube patterns
        self.shape_patterns.push(ShapePattern {
            pattern: Regex::new(r"(?i)(create|make|add|draw)\s+(?:a\s+)?(?:box|cube|rectangular\s+solid|rect)")?,
            shape_type: PrimitiveType::Box,
            parameter_extractors: vec![
                ParameterExtractor {
                    name: "width".to_string(),
                    pattern: Regex::new(r"(?i)(?:width|w)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 10.0,
                    unit_type: UnitType::Length,
                },
                ParameterExtractor {
                    name: "height".to_string(),
                    pattern: Regex::new(r"(?i)(?:height|h)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 10.0,
                    unit_type: UnitType::Length,
                },
                ParameterExtractor {
                    name: "depth".to_string(),
                    pattern: Regex::new(r"(?i)(?:depth|d|length|l)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 10.0,
                    unit_type: UnitType::Length,
                },
            ],
        });
        
        // Size-based box pattern
        self.shape_patterns.push(ShapePattern {
            pattern: Regex::new(r"(?i)(create|make|add|draw)\s+(?:a\s+)?(\d+(?:\.\d+)?)\s*(?:x|by)\s*(\d+(?:\.\d+)?)\s*(?:x|by)\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?\s*(?:box|cube)")?,
            shape_type: PrimitiveType::Box,
            parameter_extractors: vec![], // Handled differently
        });
        
        // Sphere patterns
        self.shape_patterns.push(ShapePattern {
            pattern: Regex::new(r"(?i)(create|make|add|draw)\s+(?:a\s+)?(?:sphere|ball|orb)")?,
            shape_type: PrimitiveType::Sphere,
            parameter_extractors: vec![
                ParameterExtractor {
                    name: "radius".to_string(),
                    pattern: Regex::new(r"(?i)(?:radius|r)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 5.0,
                    unit_type: UnitType::Length,
                },
                ParameterExtractor {
                    name: "diameter".to_string(),
                    pattern: Regex::new(r"(?i)(?:diameter|d)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 10.0,
                    unit_type: UnitType::Length,
                },
            ],
        });
        
        // Cylinder patterns
        self.shape_patterns.push(ShapePattern {
            pattern: Regex::new(r"(?i)(create|make|add|draw)\s+(?:a\s+)?(?:cylinder|tube|pipe|rod)")?,
            shape_type: PrimitiveType::Cylinder,
            parameter_extractors: vec![
                ParameterExtractor {
                    name: "radius".to_string(),
                    pattern: Regex::new(r"(?i)(?:radius|r)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 5.0,
                    unit_type: UnitType::Length,
                },
                ParameterExtractor {
                    name: "height".to_string(),
                    pattern: Regex::new(r"(?i)(?:height|h|length|l)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 10.0,
                    unit_type: UnitType::Length,
                },
            ],
        });
        
        // Gear patterns
        self.shape_patterns.push(ShapePattern {
            pattern: Regex::new(r"(?i)(create|make|add|draw)\s+(?:a\s+)?gear")?,
            shape_type: PrimitiveType::Gear,
            parameter_extractors: vec![
                ParameterExtractor {
                    name: "teeth".to_string(),
                    pattern: Regex::new(r"(?i)(\d+)\s*teeth")?,
                    default_value: 12.0,
                    unit_type: UnitType::Count,
                },
                ParameterExtractor {
                    name: "diameter".to_string(),
                    pattern: Regex::new(r"(?i)(?:diameter|d)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 50.0,
                    unit_type: UnitType::Length,
                },
                ParameterExtractor {
                    name: "thickness".to_string(),
                    pattern: Regex::new(r"(?i)(?:thickness|thick|t)(?:\s*[:=])?\s*(\d+(?:\.\d+)?)\s*(mm|cm|m|in)?")?,
                    default_value: 5.0,
                    unit_type: UnitType::Length,
                },
            ],
        });
        
        // Boolean operation patterns
        self.boolean_patterns.push(BooleanPattern {
            pattern: Regex::new(r"(?i)(union|combine|merge|join|add\s+together)")?,
            operation: BooleanOp::Union,
            keywords: vec!["union".to_string(), "combine".to_string(), "merge".to_string()],
        });
        
        self.boolean_patterns.push(BooleanPattern {
            pattern: Regex::new(r"(?i)(subtract|cut|remove|carve|difference)")?,
            operation: BooleanOp::Difference,
            keywords: vec!["subtract".to_string(), "cut".to_string(), "remove".to_string()],
        });
        
        self.boolean_patterns.push(BooleanPattern {
            pattern: Regex::new(r"(?i)(intersect|overlap|common|intersection)")?,
            operation: BooleanOp::Intersection,
            keywords: vec!["intersect".to_string(), "overlap".to_string()],
        });
        
        // Transform patterns
        self.transform_patterns.push(TransformPattern {
            pattern: Regex::new(r"(?i)(move|translate|shift|position)")?,
            transform_type: TransformPatternType::Move,
        });
        
        self.transform_patterns.push(TransformPattern {
            pattern: Regex::new(r"(?i)(rotate|turn|spin)")?,
            transform_type: TransformPatternType::Rotate,
        });
        
        self.transform_patterns.push(TransformPattern {
            pattern: Regex::new(r"(?i)(scale|resize|shrink|enlarge|grow)")?,
            transform_type: TransformPatternType::Scale,
        });
        
        self.transform_patterns.push(TransformPattern {
            pattern: Regex::new(r"(?i)(mirror|flip|reflect)")?,
            transform_type: TransformPatternType::Mirror,
        });
        
        Ok(())
    }
    
    /// Parse natural language command
    pub fn parse(&self, text: &str) -> CommandParseResult<AICommand> {
        let text = text.trim().to_lowercase();
        
        // Try shape patterns first
        for pattern in &self.shape_patterns {
            if pattern.pattern.is_match(&text) {
                return self.parse_shape_command(&text, pattern);
            }
        }
        
        // Try boolean operations
        for pattern in &self.boolean_patterns {
            if pattern.pattern.is_match(&text) {
                return self.parse_boolean_command(&text, pattern);
            }
        }
        
        // Try transform operations
        for pattern in &self.transform_patterns {
            if pattern.pattern.is_match(&text) {
                return self.parse_transform_command(&text, pattern);
            }
        }
        
        // Try export command
        if text.contains("export") || text.contains("save") {
            return self.parse_export_command(&text);
        }
        
        Err(CommandError::ParseError {
            message: format!("Could not understand command: {}", text),
        })
    }
    
    /// Parse shape creation command
    fn parse_shape_command(&self, text: &str, pattern: &ShapePattern) -> CommandParseResult<AICommand> {
        let mut params = HashMap::new();
        
        // Special handling for size-based box pattern
        if pattern.parameter_extractors.is_empty() && pattern.shape_type == PrimitiveType::Box {
            let size_pattern = Regex::new(r"(\d+(?:\.\d+)?)\s*(?:x|by)\s*(\d+(?:\.\d+)?)\s*(?:x|by)\s*(\d+(?:\.\d+)?)")
                .map_err(|e| CommandError::RegexError(e))?;
            
            if let Some(captures) = size_pattern.captures(text) {
                params.insert("width".to_string(), captures.get(1)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(10.0));
                params.insert("height".to_string(), captures.get(2)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(10.0));
                params.insert("depth".to_string(), captures.get(3)
                    .and_then(|m| m.as_str().parse().ok())
                    .unwrap_or(10.0));
            }
        } else {
            // Extract parameters using extractors
            for extractor in &pattern.parameter_extractors {
                if let Some(value) = self.extract_parameter(text, extractor) {
                    params.insert(extractor.name.clone(), value);
                } else {
                    params.insert(extractor.name.clone(), extractor.default_value);
                }
            }
        }
        
        // Extract position
        let position = self.extract_position(text).unwrap_or(Position { x: 0.0, y: 0.0, z: 0.0 });
        
        // Extract material
        let material = self.extract_material(text);
        
        Ok(AICommand::CreatePrimitive {
            primitive_type: pattern.shape_type.clone(),
            parameters: params,
            position,
            material,
        })
    }
    
    /// Parse boolean operation command
    fn parse_boolean_command(&self, text: &str, pattern: &BooleanPattern) -> CommandParseResult<AICommand> {
        // Extract object references
        let objects = self.extract_object_references(text);
        
        if objects.len() < 2 {
            return Err(CommandError::ParseError {
                message: "Boolean operations require at least two objects".to_string(),
            });
        }
        
        Ok(AICommand::Boolean {
            operation: pattern.operation.clone(),
            object_a: objects[0],
            object_b: objects[1],
        })
    }
    
    /// Parse transform command
    fn parse_transform_command(&self, text: &str, pattern: &TransformPattern) -> CommandParseResult<AICommand> {
        let objects = self.extract_object_references(text);
        
        if objects.is_empty() {
            return Err(CommandError::ParseError {
                message: "Transform operations require at least one object".to_string(),
            });
        }
        
        match pattern.transform_type {
            TransformPatternType::Move => {
                let delta = self.extract_vector(text).unwrap_or(Position { x: 0.0, y: 0.0, z: 0.0 });
                Ok(AICommand::Transform {
                    operation: Transform::Translate { delta },
                    targets: objects,
                })
            },
            TransformPatternType::Rotate => {
                let angle = self.extract_angle(text).unwrap_or(90.0);
                let axis = self.extract_axis(text).unwrap_or("z".to_string());
                Ok(AICommand::Transform {
                    operation: Transform::Rotate { axis, angle },
                    targets: objects,
                })
            },
            TransformPatternType::Scale => {
                let factor = self.extract_scale_factor(text).unwrap_or(2.0);
                Ok(AICommand::Transform {
                    operation: Transform::Scale { factor },
                    targets: objects,
                })
            },
            TransformPatternType::Mirror => {
                // Default to XY plane
                Ok(AICommand::Transform {
                    operation: Transform::Scale { factor: -1.0 },
                    targets: objects,
                })
            },
        }
    }
    
    /// Parse export command
    fn parse_export_command(&self, text: &str) -> CommandParseResult<AICommand> {
        let format = if text.contains("stl") {
            ExportFormat::STL
        } else if text.contains("obj") {
            ExportFormat::OBJ
        } else {
            ExportFormat::STL // Default
        };
        
        Ok(AICommand::Export {
            format,
            filename: None,
        })
    }
    
    /// Extract parameter value from text
    fn extract_parameter(&self, text: &str, extractor: &ParameterExtractor) -> Option<f64> {
        if let Some(captures) = extractor.pattern.captures(text) {
            if let Some(value_str) = captures.get(1) {
                if let Ok(value) = value_str.as_str().parse::<f64>() {
                    let unit = captures.get(2).map(|m| m.as_str()).unwrap_or("mm");
                    return Some(self.convert_units(value, unit, &extractor.unit_type));
                }
            }
        }
        
        // Special case for diameter -> radius conversion
        if extractor.name == "radius" && text.contains("diameter") {
            let patterns = PATTERNS.as_ref().ok()?;
            if let Some(captures) = patterns.number_regex.captures(text) {
                if let Some(value_str) = captures.get(1) {
                    if let Ok(diameter) = value_str.as_str().parse::<f64>() {
                        return Some(diameter / 2.0);
                    }
                }
            }
        }
        
        None
    }
    
    /// Extract position from text
    fn extract_position(&self, text: &str) -> Option<Position> {
        let pos_pattern = match Regex::new(r"at\s*\(\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*\)") {
            Ok(p) => p,
            Err(_) => return None,
        };
        
        if let Some(captures) = pos_pattern.captures(text) {
            let x = captures.get(1)?.as_str().parse().ok()?;
            let y = captures.get(2)?.as_str().parse().ok()?;
            let z = captures.get(3)?.as_str().parse().ok()?;
            return Some(Position { x, y, z });
        }
        
        // Try origin keyword
        if text.contains("origin") {
            return Some(Position { x: 0.0, y: 0.0, z: 0.0 });
        }
        
        None
    }
    
    /// Extract material from text
    fn extract_material(&self, text: &str) -> Option<String> {
        let patterns = PATTERNS.as_ref().ok()?;
        if let Some(captures) = patterns.color_regex.captures(text) {
            return captures.get(1).map(|m| m.as_str().to_string());
        }
        
        // Check for material keywords
        let materials = ["steel", "aluminum", "plastic", "glass", "wood"];
        for material in &materials {
            if text.contains(material) {
                return Some(material.to_string());
            }
        }
        
        None
    }
    
    /// Extract object references from text.
    ///
    /// Scans the input for inline canonical UUIDs (e.g. when an upstream AI
    /// provider passes object identifiers through verbatim) and returns them
    /// in the order they appear. Named or anaphoric references such as "the
    /// cube" cannot be resolved here — that requires a scene-context
    /// analyzer with access to the current model — so this function
    /// deliberately returns an empty vector when no UUIDs are present.
    ///
    /// Returning an empty vector is the correct behavior: each caller
    /// already validates that the resulting `Vec<ObjectId>` is large enough
    /// for the requested operation (e.g. boolean ops require ≥ 2) and emits
    /// a `ParseError` otherwise. Fabricating placeholder UUIDs here would
    /// silently target unrelated objects in the live scene.
    fn extract_object_references(&self, text: &str) -> Vec<ObjectId> {
        // Canonical UUID v1–v5 pattern (8-4-4-4-12 hex groups).
        static UUID_REGEX: Lazy<Result<Regex, regex::Error>> = Lazy::new(|| {
            Regex::new(
                r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b",
            )
        });

        let regex = match UUID_REGEX.as_ref() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        regex
            .find_iter(text)
            .filter_map(|m| uuid::Uuid::parse_str(m.as_str()).ok())
            .collect()
    }
    
    /// Extract vector from text
    fn extract_vector(&self, text: &str) -> Option<Position> {
        // Look for "by X, Y, Z" pattern
        let vec_pattern = match Regex::new(r"by\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)\s*,\s*(-?\d+(?:\.\d+)?)") {
            Ok(p) => p,
            Err(_) => return None,
        };
        
        if let Some(captures) = vec_pattern.captures(text) {
            let x = captures.get(1)?.as_str().parse().ok()?;
            let y = captures.get(2)?.as_str().parse().ok()?;
            let z = captures.get(3)?.as_str().parse().ok()?;
            return Some(Position { x, y, z });
        }
        
        None
    }
    
    /// Extract angle from text
    fn extract_angle(&self, text: &str) -> Option<f64> {
        let patterns = PATTERNS.as_ref().ok()?;
        if let Some(captures) = patterns.number_regex.captures(text) {
            if let Some(value_str) = captures.get(1) {
                if let Ok(value) = value_str.as_str().parse::<f64>() {
                    // Check if it's in radians
                    if text.contains("rad") {
                        return Some(value.to_degrees());
                    }
                    return Some(value);
                }
            }
        }
        None
    }
    
    /// Extract axis from text
    fn extract_axis(&self, text: &str) -> Option<String> {
        if text.contains("x axis") || text.contains("x-axis") {
            Some("x".to_string())
        } else if text.contains("y axis") || text.contains("y-axis") {
            Some("y".to_string())
        } else if text.contains("z axis") || text.contains("z-axis") {
            Some("z".to_string())
        } else {
            None
        }
    }
    
    /// Extract scale factor from text
    fn extract_scale_factor(&self, text: &str) -> Option<f64> {
        let patterns = PATTERNS.as_ref().ok()?;
        if let Some(captures) = patterns.number_regex.captures(text) {
            if let Some(value_str) = captures.get(1) {
                return value_str.as_str().parse().ok();
            }
        }
        
        // Check for keywords
        if text.contains("double") || text.contains("twice") {
            return Some(2.0);
        } else if text.contains("half") {
            return Some(0.5);
        } else if text.contains("triple") {
            return Some(3.0);
        }
        
        None
    }
    
    /// Convert units based on type
    fn convert_units(&self, value: f64, unit: &str, unit_type: &UnitType) -> f64 {
        match unit_type {
            UnitType::Length => {
                match unit {
                    "m" => value * 1000.0,
                    "cm" => value * 10.0,
                    "mm" => value,
                    "in" => value * 25.4,
                    "ft" => value * 304.8,
                    _ => value,
                }
            },
            UnitType::Angle => {
                match unit {
                    "rad" => value.to_degrees(),
                    _ => value,
                }
            },
            _ => value,
        }
    }
}

impl ContextAnalyzer {
    /// Create new context analyzer
    pub fn new() -> Self {
        Self {
            _object_references: HashMap::new(),
            _abbreviations: HashMap::new(),
            _default_units: Units::Millimeters,
        }
    }
}

impl Default for NaturalLanguageParser {
    fn default() -> Self {
        Self::new().expect("Failed to initialize parser patterns")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_parse_create_box() -> CommandParseResult<()> {
        let parser = NaturalLanguageParser::new()?;
        let result = parser.parse("create a box with width 10 height 20 depth 30")?;
        
        match result {
            AICommand::CreatePrimitive { primitive_type, parameters, .. } => {
                assert_eq!(primitive_type, PrimitiveType::Box);
                assert_eq!(parameters.get("width"), Some(&10.0));
                assert_eq!(parameters.get("height"), Some(&20.0));
                assert_eq!(parameters.get("depth"), Some(&30.0));
            },
            _ => panic!("Expected CreatePrimitive command"),
        }
        
        Ok(())
    }
    
    #[test]
    fn test_parse_sphere_with_diameter() -> CommandParseResult<()> {
        let parser = NaturalLanguageParser::new()?;
        let result = parser.parse("create a sphere with diameter 20")?;
        
        match result {
            AICommand::CreatePrimitive { primitive_type, parameters, .. } => {
                assert_eq!(primitive_type, PrimitiveType::Sphere);
                // Diameter 20 should be converted to radius 10
                assert_eq!(parameters.get("radius"), Some(&10.0));
            },
            _ => panic!("Expected CreatePrimitive command"),
        }
        
        Ok(())
    }
    
    #[test]
    fn test_parse_boolean_union() -> CommandParseResult<()> {
        let parser = NaturalLanguageParser::new()?;
        let result = parser.parse("combine the objects")?;
        
        match result {
            AICommand::Boolean { operation, .. } => {
                assert_eq!(operation, BooleanOp::Union);
            },
            _ => panic!("Expected Boolean command"),
        }
        
        Ok(())
    }
    
    #[test]
    fn test_parse_transform_move() -> CommandParseResult<()> {
        let parser = NaturalLanguageParser::new()?;
        let result = parser.parse("move the object by 10, 20, 30")?;
        
        match result {
            AICommand::Transform { operation, .. } => {
                match operation {
                    Transform::Translate { delta } => {
                        assert_eq!(delta.x, 10.0);
                        assert_eq!(delta.y, 20.0);
                        assert_eq!(delta.z, 30.0);
                    },
                    _ => panic!("Expected Translate transform"),
                }
            },
            _ => panic!("Expected Transform command"),
        }
        
        Ok(())
    }
}