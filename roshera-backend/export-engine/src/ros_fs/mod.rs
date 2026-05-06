//! # roshera_fs — Secure, AI-Native File System for Roshera (.ros v3)
//!
//! Cryptographically robust storage, audit, and IP protection for AI-driven CAD.
//!
//! ## Key Features
//! - **.ros v3 read/write support** (v3 only; no v2 compatibility shim)
//! - **AI provenance tracking**: every AI / design event is logged and auditable
//! - **Per-chunk encryption**: streaming, hardware-ready
//! - **Single Ed25519 signature per file**: provenance via signature + file mtime
//! - **Audit logs with tamper-evident chaining**
//! - **Fine-grained access control (ABAC/ACL)** with time window + MFA constraints
//!
//! ## Module Overview
//! - [`header`] — File headers, UUIDs, versioning
//! - [`chunk`] — Chunk table and in-memory chunk objects
//! - [`aipr`] — AI provenance (AI command tracking)
//! - [`keys`] — Encryption key management
//! - [`encryption`] — Chunk/file encryption and decryption
//! - [`access`] — Access control, roles, ABAC, constraints
//! - [`signature`] — Ed25519 digital signatures
//! - [`audit`] — Security/compliance audit logs
//! - [`error`] — All error types (for Result use)
//! - [`util`] — Low-level crypto, time, random, memory

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
