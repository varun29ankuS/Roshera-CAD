//! # roshera_fs — Secure, AI-Native File System for Roshera (.ros v3)
//!
//! Cryptographically robust storage, audit, and IP protection for AI-driven CAD.
//!
//! ## Key Features
//! - **Full .ros v3 read/write support** (auto-detects v2)
//! - **AI provenance tracking**: Every AI/design event can be logged and audited
//! - **Enterprise-grade encryption**: Per-chunk, streaming, hardware-ready
//! - **Digital signatures and timestamp authority (TSA)**: Provenance and public proof—**no blockchain required**
//! - **Audit logs with tamper-evident chaining**
//! - **Fine-grained, multi-level access control (ABAC/ACL)**  
//!
//! ## Legal/Compliance Provenance — Recommended Workflow
//! 1. Digitally sign your file (Ed25519/ECDSA)  
//! 2. (Optional but recommended) Obtain a timestamp authority (RFC 3161) token for your file hash  
//! 3. Store or publish the signature/timestamp for future public/legal proof
//! 4. Use the audit log for tamper-proof action history  
//!
//! > **No blockchain is required**.  
//! > All legal and public proof needs are handled with signatures + TSA.
//!
//! ## Module Overview
//! - [`header`] — File headers, UUIDs, versioning
//! - [`chunk`] — Chunk table and in-memory chunk objects
//! - [`aipr`] — AI provenance (AI command tracking)
//! - [`keys`] — Encryption key management
//! - [`encryption`] — Chunk/file encryption and decryption
//! - [`access`] — Access control, roles, ABAC, constraints
//! - [`signature`] — Digital signatures, X.509, timestamp authority
//! - [`audit`] — Security/compliance audit logs
//! - [`compat`] — v2/v3 compatibility/migration
//! - [`error`] — All error types (for Result use)
//! - [`util`] — Low-level crypto, time, random, memory

pub mod access;
pub mod aipr;
pub mod audit;
pub mod chunk;
pub mod compat;
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
pub use compat::*;
pub use encryption::*;
pub use error::{
    AccessError, AuditError, EncryptionError, FormatError, KeyManagementError, ProvenanceError,
    Result, RosFileError, SignatureError, VersionError,
};
pub use header::*;
pub use keys::*;
pub use signature::*;
pub use util::*;
