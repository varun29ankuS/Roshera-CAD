/// Natural language parser for CAD commands
///
/// # Design Rationale
/// - **Why regex patterns**: Fast, deterministic parsing for common commands
/// - **Why multi-language**: Accessibility for global users
/// - **Performance**: < 1ms parsing time
/// - **Business Value**: No cloud dependency for basic commands
use crate::commands::VoiceCommand;
use crate::providers::{CommandIntent, ParsedCommand};
use lazy_static::lazy_static;
use regex::Regex;
use shared_types::{PrimitiveType, ShapeParameters};
use std::collections::HashMap;

/// Pattern for command matching
struct Pattern {
    regex: Regex,
    command_type: CommandType,
    extractor: Box<dyn Fn(&regex::Captures) -> Option<VoiceCommand> + Send + Sync>,
}

#[derive(Debug, Clone)]
enum CommandType {
    CreateBox,
    CreateSphere,
    CreateCylinder,
    Move,
    Rotate,
    Scale,
    Extrude,
}

lazy_static! {
    static ref PATTERNS: Vec<Pattern> = vec![
        // Create box patterns
        Pattern {
            regex: Regex::new(r"(?i)create\s+(?:a\s+)?box\s+(\d+(?:\.\d+)?)\s*(?:by|x)\s*(\d+(?:\.\d+)?)\s*(?:by|x)\s*(\d+(?:\.\d+)?)").unwrap(),
            command_type: CommandType::CreateBox,
            extractor: Box::new(|caps| {
                let width = caps.get(1)?.as_str().parse().ok()?;
                let height = caps.get(2)?.as_str().parse().ok()?;
                let depth = caps.get(3)?.as_str().parse().ok()?;

                let mut params = HashMap::new();
                params.insert("width".to_string(), width);
                params.insert("height".to_string(), height);
                params.insert("depth".to_string(), depth);

                Some(VoiceCommand::Create {
                    primitive: PrimitiveType::Box,
                    parameters: ShapeParameters { params },
                    natural_text: caps.get(0)?.as_str().to_string(),
                })
            }),
        },

        // Create sphere patterns
        Pattern {
            regex: Regex::new(r"(?i)create\s+(?:a\s+)?sphere\s+(?:with\s+)?radius\s+(\d+(?:\.\d+)?)").unwrap(),
            command_type: CommandType::CreateSphere,
            extractor: Box::new(|caps| {
                let radius = caps.get(1)?.as_str().parse().ok()?;

                let mut params = HashMap::new();
                params.insert("radius".to_string(), radius);

                Some(VoiceCommand::Create {
                    primitive: PrimitiveType::Sphere,
                    parameters: ShapeParameters { params },
                    natural_text: caps.get(0)?.as_str().to_string(),
                })
            }),
        },

        // Create cylinder patterns
        Pattern {
            regex: Regex::new(r"(?i)create\s+(?:a\s+)?cylinder\s+(?:with\s+)?radius\s+(\d+(?:\.\d+)?)\s+(?:and\s+)?height\s+(\d+(?:\.\d+)?)").unwrap(),
            command_type: CommandType::CreateCylinder,
            extractor: Box::new(|caps| {
                let radius = caps.get(1)?.as_str().parse().ok()?;
                let height = caps.get(2)?.as_str().parse().ok()?;

                let mut params = HashMap::new();
                params.insert("radius".to_string(), radius);
                params.insert("height".to_string(), height);

                Some(VoiceCommand::Create {
                    primitive: PrimitiveType::Cylinder,
                    parameters: ShapeParameters { params },
                    natural_text: caps.get(0)?.as_str().to_string(),
                })
            }),
        },

        // Extrude patterns
        Pattern {
            regex: Regex::new(r"(?i)extrude(?:\s+face)?(?:\s+(\d+))?(?:\s+by\s+(\d+(?:\.\d+)?))?").unwrap(),
            command_type: CommandType::Extrude,
            extractor: Box::new(|caps| {
                let face_index = caps.get(1).and_then(|m| m.as_str().parse().ok());
                let distance = caps.get(2).and_then(|m| m.as_str().parse().ok());

                Some(VoiceCommand::Extrude {
                    target: None, // Will be set by context
                    face_index,
                    direction: None, // Will prompt user
                    distance,
                    natural_text: caps.get(0)?.as_str().to_string(),
                })
            }),
        },
    ];
}

/// Simple pattern-based parser
pub struct SimpleParser;

impl SimpleParser {
    /// Create new parser
    pub fn new() -> Self {
        Self
    }

    /// Parse text to voice command
    pub fn parse(&self, text: &str) -> Result<VoiceCommand, ParseError> {
        // Try each pattern
        for pattern in PATTERNS.iter() {
            if let Some(captures) = pattern.regex.captures(text) {
                if let Some(command) = (pattern.extractor)(&captures) {
                    return Ok(command);
                }
            }
        }

        Err(ParseError::NoMatchingPattern(text.to_string()))
    }

    /// Parse text to ParsedCommand (for AI processor compatibility)
    pub fn parse_to_intent(&self, text: &str) -> Result<ParsedCommand, ParseError> {
        let voice_cmd = self.parse(text)?;

        let (intent, params) = match voice_cmd {
            VoiceCommand::Extrude {
                face_index,
                distance,
                ..
            } => {
                let mut params = HashMap::new();
                if let Some(idx) = face_index {
                    params.insert("face_index".to_string(), serde_json::json!(idx));
                }
                if let Some(dist) = distance {
                    params.insert("distance".to_string(), serde_json::json!(dist));
                }
                (CommandIntent::Extrude { target: None }, params)
            }
            _ => {
                // For other commands, return Unknown for now
                (CommandIntent::Unknown, HashMap::new())
            }
        };

        Ok(ParsedCommand {
            intent,
            parameters: params,
            confidence: 0.9,
            language: "en".to_string(),
            original_text: text.to_string(),
        })
    }
}

/// Parse errors
#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("No matching pattern for: {0}")]
    NoMatchingPattern(String),

    #[error("Invalid parameters in command")]
    InvalidParameters,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_create_box() {
        let parser = SimpleParser::new();
        let result = parser.parse("create a box 10 by 20 by 30").unwrap();

        match result {
            VoiceCommand::Create {
                primitive,
                parameters,
                ..
            } => {
                assert_eq!(primitive, PrimitiveType::Box);
                assert_eq!(parameters.params.get("width"), Some(&10.0));
                assert_eq!(parameters.params.get("height"), Some(&20.0));
                assert_eq!(parameters.params.get("depth"), Some(&30.0));
            }
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_parse_create_sphere() {
        let parser = SimpleParser::new();
        let result = parser.parse("create sphere with radius 5").unwrap();

        match result {
            VoiceCommand::Create {
                primitive,
                parameters,
                ..
            } => {
                assert_eq!(primitive, PrimitiveType::Sphere);
                assert_eq!(parameters.params.get("radius"), Some(&5.0));
            }
            _ => panic!("Wrong command type"),
        }
    }

    #[test]
    fn test_parse_invalid() {
        let parser = SimpleParser::new();
        let result = parser.parse("do something weird");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_extrude() {
        let parser = SimpleParser::new();

        // Test basic extrude
        let result = parser.parse("extrude").unwrap();
        match result {
            VoiceCommand::Extrude {
                face_index,
                distance,
                ..
            } => {
                assert_eq!(face_index, None);
                assert_eq!(distance, None);
            }
            _ => panic!("Wrong command type"),
        }

        // Test extrude with face
        let result = parser.parse("extrude face 2").unwrap();
        match result {
            VoiceCommand::Extrude {
                face_index,
                distance,
                ..
            } => {
                assert_eq!(face_index, Some(2));
                assert_eq!(distance, None);
            }
            _ => panic!("Wrong command type"),
        }

        // Test extrude with distance
        let result = parser.parse("extrude by 5.5").unwrap();
        match result {
            VoiceCommand::Extrude {
                face_index,
                distance,
                ..
            } => {
                assert_eq!(face_index, None);
                assert_eq!(distance, Some(5.5));
            }
            _ => panic!("Wrong command type"),
        }

        // Test full extrude command
        let result = parser.parse("extrude face 3 by 10").unwrap();
        match result {
            VoiceCommand::Extrude {
                face_index,
                distance,
                ..
            } => {
                assert_eq!(face_index, Some(3));
                assert_eq!(distance, Some(10.0));
            }
            _ => panic!("Wrong command type"),
        }
    }
}
