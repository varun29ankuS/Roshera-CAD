//! Handler modules for the API server
//!
//! This module contains all the HTTP handler implementations for different
//! functionalities of the Roshera CAD system.

pub mod agent;
pub mod auth;
pub mod capabilities;
pub mod datums;
pub mod document;
pub mod export;
pub mod geometry;
pub mod hierarchy;
pub mod session;
pub mod timeline;

// AUDIT-M6: `handlers::cache` and `handlers::scene` removed — both
// modules were fully orphan (zero refs outside the def-file even though
// each handler was `pub use`d via the glob below). The router never
// mounted a single one of their endpoints, so they had no runtime path.

// Re-export commonly used handlers
pub use auth::*;
pub use capabilities::*;
pub use export::*;
pub use geometry::*;
pub use hierarchy::*;
pub use session::*;
pub use timeline::*;
