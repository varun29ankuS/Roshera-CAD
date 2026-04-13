//! Timeline Engine - Event-sourced history management for CAD
//!
//! This crate provides a modern alternative to traditional version control
//! for CAD systems, using an event-sourced timeline approach that naturally
//! supports AI-driven design exploration and real-time collaboration.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod timeline;
pub mod timeline_impl;
pub mod types;

// Re-export timeline_impl types
pub use timeline_impl::{BranchInfo, ReplayResult, TimelineState};
pub mod branch;
pub mod brep_serialization;
pub mod cache;
pub mod dependency_graph;
pub mod entity_mapping;
pub mod execution;
pub mod operations;
pub mod storage;

// Re-export commonly used types
pub use branch::{BranchManager, MergeResult, MergeStrategy};
pub use cache::{CacheConfig, CacheManager};
pub use dependency_graph::DependencyGraph;
pub use error::{TimelineError, TimelineResult};
pub use execution::{ExecutionConfig, ExecutionEngine, OperationImpl};
pub use timeline::Timeline;
pub use types::*;

/// Timeline engine configuration
pub use types::TimelineConfig;

/// Prelude for convenient imports
pub mod prelude {
    pub use crate::{
        Author, Branch, BranchId, BranchPurpose, BranchState, Checkpoint, DependencyType, EntityId,
        EventId, Operation, SessionId, Timeline, TimelineConfig, TimelineError, TimelineEvent,
        TimelineResult,
    };
}
