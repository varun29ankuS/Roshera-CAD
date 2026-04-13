// ai-integration/src/commands.rs

use serde::{Deserialize, Serialize};
use shared_types::{PrimitiveType, ShapeParameters};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VoiceCommand {
    Create {
        primitive: PrimitiveType,    // Use shared type
        parameters: ShapeParameters, // Use shared type
        natural_text: String,        // Keep original for learning
    },
    // New variant for Part Maturity workflow - ensures consistency
    ActivatePartMaturityWorkflow {
        primitive: PrimitiveType,
        parameters: ShapeParameters,
        sketch_plane: String, // "XY", "XZ", "YZ" - default to XY
        natural_text: String,
    },
    Modify {
        target: Uuid,
        operation: Operation,
        parameters: HashMap<String, f64>,
    },
    Query {
        question: String,
        target: Option<Uuid>,
    },
    Extrude {
        target: Option<Uuid>,
        face_index: Option<u32>,
        direction: Option<[f64; 3]>,
        distance: Option<f64>,
        natural_text: String,
    },
}

// Removed duplicate PrimitiveType definition; using shared_types::PrimitiveType instead.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Move { x: f64, y: f64, z: f64 },
    Rotate { axis: String, angle: f64 },
    Scale { factor: f64 },
}

// Error type
#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),
    #[error("Target not found: {0}")]
    TargetNotFound(Uuid),
    #[error("Operation failed: {0}")]
    OperationFailed(String),
    #[error("Parse error: {message}")]
    ParseError { message: String },
    #[error("Regex compilation error: {0}")]
    RegexError(#[from] regex::Error),
}

// Result type alias
pub type CommandParseResult<T> = Result<T, CommandError>;
