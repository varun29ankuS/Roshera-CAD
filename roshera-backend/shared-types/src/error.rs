//! Error types for Roshera CAD
//!
//! Provides a comprehensive error hierarchy for all operations in the system.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Top-level error type for all Roshera operations
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum RosheraError {
    /// Geometry-related errors
    #[error("Geometry error: {0}")]
    Geometry(#[from] GeometryError),

    /// Command processing errors
    #[error("Command error: {0}")]
    Command(#[from] CommandError),

    /// Session management errors
    #[error("Session error: {0}")]
    Session(#[from] SessionError),

    /// API-related errors
    #[error("API error: {0}")]
    Api(#[from] ApiError),

    /// Export operation errors
    #[error("Export error: {0}")]
    Export(#[from] ExportError),

    /// Generic internal error
    #[error("Internal error: {msg}")]
    Internal { msg: String },
}

/// Errors related to geometry operations
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum GeometryError {
    /// Invalid mesh structure
    #[error("Invalid mesh: {reason}")]
    InvalidMesh { reason: String },

    /// Boolean operation failure
    #[error("Boolean operation '{operation}' failed: {reason}")]
    BooleanFailed { operation: String, reason: String },

    /// Invalid shape parameters
    #[error("Invalid parameters for shape: {param}")]
    InvalidParameters { param: String },

    /// Mesh is not manifold
    #[error("Mesh is not manifold: {details}")]
    NonManifold { details: String },

    /// Tessellation failure
    #[error("Tessellation failed: {reason}")]
    TessellationFailed { reason: String },

    /// Transformation error
    #[error("Transform error: {reason}")]
    TransformError { reason: String },

    /// Validation error
    #[error("Validation failed: {reason}")]
    ValidationError { reason: String },

    /// Invalid geometry ID
    #[error("Invalid geometry ID '{id}': {reason}")]
    InvalidGeometryId { id: String, reason: String },
}

/// Errors related to command processing
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum CommandError {
    /// Command parsing error
    #[error("Failed to parse command: {msg}")]
    ParseError { msg: String },

    /// Command execution failure
    #[error("Command execution failed: {reason}")]
    ExecutionFailed { reason: String },

    /// Invalid command parameters
    #[error("Invalid parameters: {field}")]
    ValidationFailed { field: String },

    /// Unknown command type
    #[error("Unknown command: {command}")]
    UnknownCommand { command: String },

    /// Command timeout
    #[error("Command timed out after {seconds} seconds")]
    Timeout { seconds: u64 },

    /// Permission denied
    #[error("Permission denied: {action}")]
    PermissionDenied { action: String },
}

/// Errors related to session management
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum SessionError {
    /// Session not found
    #[error("Session not found: {id}")]
    NotFound { id: String },

    /// Access denied to session
    #[error("Access denied to session")]
    AccessDenied,

    /// Session has expired
    #[error("Session expired: {id}")]
    Expired { id: String },

    /// History operation failed
    #[error("History operation failed: {operation}")]
    HistoryFailed { operation: String },

    /// Collaboration conflict
    #[error("Collaboration conflict: {details}")]
    ConflictError { details: String },

    /// Persistence error
    #[error("Failed to persist session: {reason}")]
    PersistenceError { reason: String },

    /// Invalid input parameter
    #[error("Invalid input parameter: {field}")]
    InvalidInput { field: String },

    /// Rate limit exceeded
    #[error("Rate limit exceeded")]
    RateLimitExceeded,
}

/// Errors related to API operations
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum ApiError {
    /// Bad request
    #[error("Bad request: {msg}")]
    BadRequest { msg: String },

    /// Internal server error
    #[error("Internal server error")]
    InternalServerError,

    /// Resource not found
    #[error("Resource not found: {resource}")]
    NotFound { resource: String },

    /// Request timeout
    #[error("Request timed out")]
    Timeout,

    /// Rate limit exceeded
    #[error("Rate limit exceeded: {limit} requests per {window}")]
    RateLimitExceeded { limit: u32, window: String },

    /// Authentication required
    #[error("Authentication required")]
    Unauthorized,

    /// Invalid content type
    #[error("Invalid content type: expected {expected}, got {actual}")]
    InvalidContentType { expected: String, actual: String },
}

/// Errors related to export operations
#[derive(Error, Debug, Clone, Serialize, Deserialize)]
pub enum ExportError {
    /// Unsupported export format
    #[error("Unsupported export format: {format}")]
    UnsupportedFormat { format: String },

    /// Export operation failed
    #[error("Export failed: {reason}")]
    ExportFailed { reason: String },

    /// File write error
    #[error("Failed to write file: {path}")]
    FileWriteError { path: String },

    /// File read error
    #[error("Failed to read file: {path}")]
    FileReadError { path: String },

    /// Invalid mesh for export
    #[error("Mesh is invalid for export: {reason}")]
    InvalidMesh { reason: String },

    /// Export size limit exceeded
    #[error("Export size limit exceeded: {size_mb}MB > {limit_mb}MB")]
    SizeLimitExceeded { size_mb: f64, limit_mb: f64 },
}

impl RosheraError {
    /// Get the error code for API responses
    pub fn error_code(&self) -> u32 {
        match self {
            RosheraError::Geometry(_) => 1000,
            RosheraError::Command(_) => 2000,
            RosheraError::Session(_) => 3000,
            RosheraError::Api(_) => 4000,
            RosheraError::Export(_) => 5000,
            RosheraError::Internal { .. } => 9999,
        }
    }

    /// Check if the error is retryable
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            RosheraError::Api(ApiError::Timeout)
                | RosheraError::Api(ApiError::InternalServerError)
                | RosheraError::Command(CommandError::Timeout { .. })
        )
    }
}

/// Conversion from serde_json::Error to SessionError
impl From<serde_json::Error> for SessionError {
    fn from(err: serde_json::Error) -> Self {
        SessionError::PersistenceError {
            reason: format!("JSON serialization error: {}", err),
        }
    }
}

/// Conversion from uuid::Error to SessionError
impl From<uuid::Error> for SessionError {
    fn from(err: uuid::Error) -> Self {
        SessionError::InvalidInput {
            field: format!("UUID parsing error: {}", err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_codes() {
        let geometry_error = RosheraError::from(GeometryError::InvalidMesh {
            reason: "test".to_string(),
        });
        assert_eq!(geometry_error.error_code(), 1000);

        let api_error = RosheraError::from(ApiError::Timeout);
        assert_eq!(api_error.error_code(), 4000);
        assert!(api_error.is_retryable());
    }
}
