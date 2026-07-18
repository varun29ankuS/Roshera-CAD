//! Timeline Engine - Event-sourced history management for CAD
//!
//! This crate provides a modern alternative to traditional version control
//! for CAD systems, using an event-sourced timeline approach that naturally
//! supports AI-driven design exploration and real-time collaboration.

// Reason: the workspace denies unwrap/expect/panic in PRODUCTION code (this
// attribute is inert outside `cfg(test)`). In unit tests, panicking is the
// test framework's failure mechanism. Enforced since CI clippy exit-code
// hardening (tasks #43/#53).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
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
pub mod dependency_projection;
pub mod entity_mapping;
pub mod execution;
pub mod incremental;
pub mod mould;
pub mod operations;
pub mod rebuild_certificate;
pub mod recorder_bridge;
pub mod replay;
pub mod storage;

// Re-export commonly used types
pub use branch::{
    suggest_branch_names, BranchManager, MergeResult, MergeStrategy, BRANCH_NAME_POOL,
};
pub use cache::{CacheConfig, CacheManager};
pub use dependency_graph::DependencyGraph;
pub use dependency_projection::build_dependency_graph;
pub use error::{TimelineError, TimelineResult};
pub use execution::{ExecutionConfig, ExecutionEngine, OperationImpl};
pub use incremental::{
    incremental_rebuild, incremental_rebuild_verified, IncrementalStats, ModelDigest, PrefixCache,
};
pub use mould::{
    is_param_meta, mould_operation, name_binding_operation, params_have_numeric, NameBindings,
    OverrideSet, MOULD_COMMAND, NAME_COMMAND,
};
pub use rebuild_certificate::{certify_rebuild, FeatureStatus, FeatureVerdict, RebuildCertificate};
pub use recorder_bridge::{SharedTimeline, TimelineRecorder};
pub use replay::{apply_event, rebuild_model_from_events, ReplayError, ReplayOutcome};
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
