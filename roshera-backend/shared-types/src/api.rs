//! API request and response types
//!
//! Defines the data structures for HTTP API communication.

use crate::{
    BooleanOp, CADObject, CommandResult, ExportFormat, ExportOptions, ObjectId, PrimitiveType,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Request to create geometry
#[derive(Debug, Serialize, Deserialize)]
pub struct GeometryCreateRequest {
    /// Type of shape to create
    pub shape_type: PrimitiveType,
    /// Shape parameters (flattened)
    #[serde(flatten)]
    pub parameters: HashMap<String, f64>,
    /// Position in 3D space
    pub position: [f32; 3],
    /// Optional material name
    pub material: Option<String>,
}

/// Response for geometry creation
#[derive(Debug, Serialize, Deserialize)]
pub struct GeometryResponse {
    /// Created object
    pub object: CADObject,
    /// Success flag
    pub success: bool,
    /// Execution time in milliseconds
    pub execution_time_ms: u64,
    /// Additional message
    pub message: String,
}

/// Request for boolean operation
#[derive(Debug, Serialize, Deserialize)]
pub struct BooleanRequest {
    /// Type of operation
    pub operation: BooleanOp,
    /// Object IDs to operate on
    pub objects: Vec<ObjectId>,
    /// Keep original objects
    pub keep_originals: bool,
}

/// Response for boolean operation
#[derive(Debug, Serialize, Deserialize)]
pub struct BooleanResponse {
    /// Resulting object
    pub result_object: CADObject,
    /// Success flag
    pub success: bool,
    /// Execution time
    pub execution_time_ms: u64,
    /// Input objects used
    pub input_objects: Vec<ObjectId>,
}

/// Request for natural language command
#[derive(Debug, Serialize, Deserialize)]
pub struct NaturalLanguageRequest {
    /// Natural language command text
    pub command: String,
    /// Session to execute in
    pub session_id: ObjectId,
    /// Optional context hints
    pub context: Option<HashMap<String, serde_json::Value>>,
}

/// Response for natural language command
#[derive(Debug, Serialize, Deserialize)]
pub struct NaturalLanguageResponse {
    /// Results of executed commands
    pub results: Vec<CommandResult>,
    /// Overall success
    pub success: bool,
    /// Total processing time
    pub processing_time_ms: u64,
    /// Parsed commands (for debugging)
    pub parsed_commands: Option<Vec<String>>,
}

/// Request to export geometry
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportRequest {
    /// Export format
    pub format: ExportFormat,
    /// Objects to export (empty = all)
    pub objects: Vec<ObjectId>,
    /// Export options
    #[serde(default)]
    pub options: ExportOptions,
}

/// Response for export operation
#[derive(Debug, Serialize, Deserialize)]
pub struct ExportResponse {
    /// Generated filename
    pub filename: String,
    /// File size in bytes
    pub file_size: u64,
    /// Export format used
    pub format: ExportFormat,
    /// Success flag
    pub success: bool,
    /// Export time
    pub export_time_ms: u64,
    /// Download URL
    pub download_url: String,
}

/// Generic error response
#[derive(Debug, Serialize, Deserialize)]
pub struct ErrorResponse {
    /// Error message
    pub error: String,
    /// Error code
    pub code: u32,
    /// Timestamp
    pub timestamp: u64,
    /// Request ID for tracking
    pub request_id: Option<String>,
}

/// Health check response
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthResponse {
    /// Service status
    pub status: String,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Active sessions count
    pub active_sessions: u32,
    /// Version information
    pub version: String,
    /// Additional health metrics
    pub metrics: HealthMetrics,
}

/// Health metrics
#[derive(Debug, Serialize, Deserialize)]
pub struct HealthMetrics {
    /// Memory usage in MB
    pub memory_usage_mb: f64,
    /// CPU usage percentage
    pub cpu_usage_percent: f64,
    /// Request rate per second
    pub requests_per_second: f64,
    /// Average response time in ms
    pub avg_response_time_ms: f64,
}

/// Session creation request
#[derive(Debug, Serialize, Deserialize)]
pub struct CreateSessionRequest {
    /// User name
    pub user_name: String,
    /// Session name (optional)
    pub session_name: Option<String>,
    /// Initial settings
    pub settings: Option<HashMap<String, serde_json::Value>>,
}

/// Session response
#[derive(Debug, Serialize, Deserialize)]
pub struct SessionResponse {
    /// Session ID
    pub id: ObjectId,
    /// Session name
    pub name: String,
    /// Creation timestamp
    pub created_at: u64,
    /// Number of objects
    pub object_count: usize,
    /// Active users
    pub user_count: usize,
}

/// Batch operation request
#[derive(Debug, Serialize, Deserialize)]
pub struct BatchRequest {
    /// Commands to execute
    pub commands: Vec<serde_json::Value>,
    /// Execute in parallel
    pub parallel: bool,
    /// Stop on first error
    pub stop_on_error: bool,
}

/// Batch operation response
#[derive(Debug, Serialize, Deserialize)]
pub struct BatchResponse {
    /// Results for each command
    pub results: Vec<BatchResult>,
    /// Total execution time
    pub total_time_ms: u64,
    /// Number of successes
    pub success_count: usize,
    /// Number of failures
    pub failure_count: usize,
}

/// Individual batch result
#[derive(Debug, Serialize, Deserialize)]
pub struct BatchResult {
    /// Command index
    pub index: usize,
    /// Success flag
    pub success: bool,
    /// Result data
    pub result: Option<serde_json::Value>,
    /// Error message if failed
    pub error: Option<String>,
    /// Execution time
    pub time_ms: u64,
}

impl ErrorResponse {
    /// Create error response
    pub fn new(error: impl Into<String>, code: u32) -> Self {
        Self {
            error: error.into(),
            code,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis() as u64,
            request_id: None,
        }
    }

    /// Add request ID
    pub fn with_request_id(mut self, id: String) -> Self {
        self.request_id = Some(id);
        self
    }
}

impl Default for HealthMetrics {
    fn default() -> Self {
        Self {
            memory_usage_mb: 0.0,
            cpu_usage_percent: 0.0,
            requests_per_second: 0.0,
            avg_response_time_ms: 0.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_response() {
        let error = ErrorResponse::new("Test error", 1001).with_request_id("req-123".to_string());

        assert_eq!(error.error, "Test error");
        assert_eq!(error.code, 1001);
        assert!(error.request_id.is_some());
    }

    #[test]
    fn test_serialization() {
        let request = GeometryCreateRequest {
            shape_type: PrimitiveType::Box,
            parameters: HashMap::from([
                ("width".to_string(), 10.0),
                ("height".to_string(), 5.0),
                ("depth".to_string(), 3.0),
            ]),
            position: [0.0, 0.0, 0.0],
            material: Some("steel".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"shape_type\":\"Box\""));
        assert!(json.contains("\"width\":10.0"));
    }
}
