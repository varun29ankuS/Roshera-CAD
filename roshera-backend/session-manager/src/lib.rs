//! Session management for multi-user CAD collaboration
//!
//! This crate provides session state management, persistence, and real-time
//! collaboration features for the Roshera CAD system.

// Reason: the workspace denies unwrap/expect/panic in PRODUCTION code (this
// attribute is inert outside `cfg(test)`). In unit tests, panicking is the
// test framework's failure mechanism. Enforced since CI clippy exit-code
// hardening (tasks #43/#53).
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used, clippy::panic))]
#![forbid(unsafe_code)]
#![warn(missing_docs)]
#![warn(clippy::all)]

pub mod auth;
pub mod broadcast;
pub mod cache;
pub mod collaboration;
pub mod command_processor;
pub mod conflict_resolution;
pub mod database;
pub mod delta;
pub mod delta_manager;
pub mod hierarchy_manager;
pub mod manager;
pub mod permissions;
pub mod persistence;
pub mod state;
pub mod timeline_integration;

pub use auth::*;
pub use broadcast::*;
pub use cache::*;
pub use collaboration::*;
pub use conflict_resolution::*;
pub use database::*;
pub use delta::*;
pub use delta_manager::*;
pub use hierarchy_manager::*;
pub use manager::*;
pub use permissions::*;
pub use persistence::*;
pub use state::*;

use shared_types::*;

// Re-export commonly used types from shared_types for convenience
pub use shared_types::SessionError;
pub use shared_types::SessionState;
