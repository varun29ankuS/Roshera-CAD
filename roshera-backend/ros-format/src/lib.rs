//! # ros-format — Roshera .ros v3.1 file format core
//!
//! Geometry-free building blocks for the .ros file format: header,
//! chunk table, encryption, key management, signatures, audit log,
//! merkle tree, access control, and AI provenance (PROV) chunk.
//!
//! Lifted out of `export-engine` in slice 4 so bridge/SDK consumers
//! can read or write .ros files (or just AIPR-only audit logs)
//! without dragging in the geometry kernel. The HIST chunk that
//! carries `timeline_engine::TimelineEvent` lives in `export-engine`
//! because it depends on the kernel-coupled timeline crate.
//!
//! ## Module Overview
//! - [`header`] — File headers, UUIDs, versioning
//! - [`chunk`] — Chunk table and in-memory chunk objects
//! - [`aipr`] — AI provenance (PROV chunk + command tracking)
//! - [`keys`] — Encryption key management
//! - [`encryption`] — Chunk/file encryption and decryption
//! - [`access`] — Access control, roles, ABAC, constraints
//! - [`signature`] — Ed25519 digital signatures
//! - [`audit`] — Security/compliance audit logs
//! - [`merkle`] — Tamper-evident hash trees
//! - [`error`] — All error types (for Result use)
//! - [`util`] — Low-level crypto, time, random, memory

#![forbid(unsafe_code)]

pub mod access;
pub mod aipr;
pub mod audit;
pub mod chunk;
pub mod encryption;
pub mod error;
pub mod header;
pub mod keys;
pub mod merkle;
pub mod signature;
pub mod util;

// Main re-exports for easy access
pub use access::*;
pub use aipr::*;
pub use audit::*;
pub use chunk::*;
pub use encryption::*;
pub use error::{
    AccessError, AuditError, EncryptionError, FormatError, KeyManagementError, ProvenanceError,
    Result, RosFileError, SignatureError, VersionError,
};
pub use header::*;
pub use keys::*;
pub use signature::*;
pub use util::*;
