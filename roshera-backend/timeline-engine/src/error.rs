//! Error types for the Timeline Engine

use crate::{BranchId, CheckpointId, EntityId, EventId};
use std::io;
use thiserror::Error;

/// Main error type for timeline operations
#[derive(Error, Debug)]
pub enum TimelineError {
    /// Event not found
    #[error("Event not found: {0}")]
    EventNotFound(EventId),

    /// Branch not found
    #[error("Branch not found: {0}")]
    BranchNotFound(BranchId),

    /// Entity not found
    #[error("Entity not found: {0}")]
    EntityNotFound(EntityId),

    /// Checkpoint not found
    #[error("Checkpoint not found: {0}")]
    CheckpointNotFound(CheckpointId),

    /// Invalid operation
    #[error("Invalid operation: {0}")]
    InvalidOperation(String),

    /// Dependency violation
    #[error("Dependency violation: {0}")]
    DependencyViolation(String),

    /// Concurrent modification
    #[error("Concurrent modification detected: {0}")]
    ConcurrentModification(String),

    /// Branch conflict
    #[error("Branch conflict: {0}")]
    BranchConflict(String),

    /// Merge error
    #[error("Merge error: {0}")]
    MergeError(String),

    /// Storage error
    #[error("Storage error: {0}")]
    StorageError(#[from] io::Error),

    /// Serialization error
    #[error("Serialization error: {0}")]
    SerializationError(String),

    /// Deserialization error
    #[error("Deserialization error: {0}")]
    DeserializationError(String),

    /// JSON error
    #[error("JSON error: {0}")]
    JsonError(#[from] serde_json::Error),

    /// Validation error
    #[error("Validation error: {0}")]
    ValidationError(String),

    /// Execution error
    #[error("Execution error: {0}")]
    ExecutionError(String),

    /// Execution failed
    #[error("Execution failed: {0}")]
    ExecutionFailed(String),

    /// Geometry engine error
    #[error("Geometry engine error: {0}")]
    GeometryError(String),

    /// Cache error
    #[error("Cache error: {0}")]
    CacheError(String),

    /// Configuration error
    #[error("Configuration error: {0}")]
    ConfigError(String),

    /// Not implemented
    #[error("Not implemented: {0}")]
    NotImplemented(String),

    /// Generic internal error
    #[error("Internal error: {0}")]
    Internal(String),

    /// Session not found
    #[error("Session not found")]
    SessionNotFound,

    /// No more operations to undo
    #[error("No more operations to undo")]
    NoMoreUndo,

    /// No more operations to redo
    #[error("No more operations to redo")]
    NoMoreRedo,
}

/// Result type alias for timeline operations
pub type TimelineResult<T> = Result<T, TimelineError>;

/// Extension trait for converting other errors
pub trait IntoTimelineError {
    /// Convert to timeline error
    fn into_timeline_error(self) -> TimelineError;
}

impl IntoTimelineError for String {
    fn into_timeline_error(self) -> TimelineError {
        TimelineError::Internal(self)
    }
}

impl IntoTimelineError for &str {
    fn into_timeline_error(self) -> TimelineError {
        TimelineError::Internal(self.to_string())
    }
}

/// Helper for validation errors
pub fn validation_error<T>(msg: impl Into<String>) -> TimelineResult<T> {
    Err(TimelineError::ValidationError(msg.into()))
}

/// Helper for execution errors
pub fn execution_error<T>(msg: impl Into<String>) -> TimelineResult<T> {
    Err(TimelineError::ExecutionError(msg.into()))
}

/// Helper for not implemented errors
pub fn not_implemented<T>(feature: impl Into<String>) -> TimelineResult<T> {
    Err(TimelineError::NotImplemented(feature.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = TimelineError::EventNotFound(EventId::new());
        assert!(err.to_string().contains("Event not found"));
    }

    #[test]
    fn test_error_conversion() {
        let err: TimelineError = "test error".into_timeline_error();
        match err {
            TimelineError::Internal(msg) => assert_eq!(msg, "test error"),
            _ => panic!("Wrong error type"),
        }
    }
}
