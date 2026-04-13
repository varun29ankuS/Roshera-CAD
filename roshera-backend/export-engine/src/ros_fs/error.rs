// src/error.rs

//! Central Error Types for Roshera File System
//!
//! Provides comprehensive error handling with:
//! - Detailed error context
//! - Error chaining for debugging
//! - Conversion traits for common error types
//! - Actionable error messages

use std::error::Error;
use std::fmt;
use std::io;

/// Main result type for the ROS file system module
pub type Result<T> = std::result::Result<T, RosFileError>;

/// Top-level ROS file system error enum
#[derive(Debug)]
pub enum RosFileError {
    /// I/O related errors
    Io(io::Error),

    /// Encryption/decryption errors
    Encryption(EncryptionError),

    /// Access control violations
    Access(AccessError),

    /// AI provenance tracking errors
    Provenance(ProvenanceError),

    /// Digital signature errors
    Signature(SignatureError),

    /// Key management errors
    KeyManagement(KeyManagementError),

    /// Audit log errors
    Audit(AuditError),

    /// File format errors
    Format(FormatError),

    /// Version compatibility errors
    Version(VersionError),

    /// Resource not found
    NotFound { resource: String, context: String },

    /// Operation not supported
    Unsupported { operation: String, reason: String },

    /// Generic error with context
    Other {
        message: String,
        source: Option<Box<dyn Error + Send + Sync>>,
    },
}

impl Error for RosFileError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            RosFileError::Io(e) => Some(e),
            RosFileError::Encryption(e) => Some(e),
            RosFileError::Access(e) => Some(e),
            RosFileError::Provenance(e) => Some(e),
            RosFileError::Signature(e) => Some(e),
            RosFileError::KeyManagement(e) => Some(e),
            RosFileError::Audit(e) => Some(e),
            RosFileError::Format(e) => Some(e),
            RosFileError::Version(e) => Some(e),
            RosFileError::Other { source, .. } => source.as_ref().map(|s| s.as_ref() as &dyn Error),
            _ => None,
        }
    }
}

impl fmt::Display for RosFileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RosFileError::Io(e) => write!(f, "I/O error: {}", e),
            RosFileError::Encryption(e) => write!(f, "Encryption error: {}", e),
            RosFileError::Access(e) => write!(f, "Access control error: {}", e),
            RosFileError::Provenance(e) => write!(f, "AI provenance error: {}", e),
            RosFileError::Signature(e) => write!(f, "Signature error: {}", e),
            RosFileError::KeyManagement(e) => write!(f, "Key management error: {}", e),
            RosFileError::Audit(e) => write!(f, "Audit error: {}", e),
            RosFileError::Format(e) => write!(f, "Format error: {}", e),
            RosFileError::Version(e) => write!(f, "Version error: {}", e),
            RosFileError::NotFound { resource, context } => {
                write!(f, "Resource not found: {} (context: {})", resource, context)
            }
            RosFileError::Unsupported { operation, reason } => {
                write!(
                    f,
                    "Unsupported operation: {} (reason: {})",
                    operation, reason
                )
            }
            RosFileError::Other { message, .. } => write!(f, "{}", message),
        }
    }
}

// Conversion implementations
impl From<io::Error> for RosFileError {
    fn from(e: io::Error) -> Self {
        RosFileError::Io(e)
    }
}

impl From<EncryptionError> for RosFileError {
    fn from(e: EncryptionError) -> Self {
        RosFileError::Encryption(e)
    }
}

impl From<AccessError> for RosFileError {
    fn from(e: AccessError) -> Self {
        RosFileError::Access(e)
    }
}

impl From<ProvenanceError> for RosFileError {
    fn from(e: ProvenanceError) -> Self {
        RosFileError::Provenance(e)
    }
}

impl From<SignatureError> for RosFileError {
    fn from(e: SignatureError) -> Self {
        RosFileError::Signature(e)
    }
}

impl From<KeyManagementError> for RosFileError {
    fn from(e: KeyManagementError) -> Self {
        RosFileError::KeyManagement(e)
    }
}

impl From<AuditError> for RosFileError {
    fn from(e: AuditError) -> Self {
        RosFileError::Audit(e)
    }
}

impl From<FormatError> for RosFileError {
    fn from(e: FormatError) -> Self {
        RosFileError::Format(e)
    }
}

impl From<VersionError> for RosFileError {
    fn from(e: VersionError) -> Self {
        RosFileError::Version(e)
    }
}

/// Encryption and decryption errors
#[derive(Debug, Clone)]
pub enum EncryptionError {
    /// Key not found for the specified operation
    MissingKey { key_id: String },

    /// Encryption operation failed
    EncryptionFailed { algorithm: String, details: String },

    /// Decryption operation failed
    DecryptionFailed { algorithm: String, details: String },

    /// Unsupported encryption algorithm
    UnsupportedAlgorithm {
        algorithm: String,
        supported: Vec<String>,
    },

    /// Access denied due to insufficient permissions
    AccessDenied {
        required_level: u32,
        current_level: u32,
    },

    /// Data corruption detected
    CorruptedData {
        expected_tag: String,
        actual_tag: String,
    },

    /// Invalid initialization vector
    InvalidIv {
        expected_len: usize,
        actual_len: usize,
    },

    /// Key derivation failed
    KeyDerivationFailed { reason: String },
}

impl fmt::Display for EncryptionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use EncryptionError::*;
        match self {
            MissingKey { key_id } => write!(f, "Missing encryption key: {}", key_id),
            EncryptionFailed { algorithm, details } => {
                write!(f, "Encryption failed ({}): {}", algorithm, details)
            }
            DecryptionFailed { algorithm, details } => {
                write!(f, "Decryption failed ({}): {}", algorithm, details)
            }
            UnsupportedAlgorithm {
                algorithm,
                supported,
            } => {
                write!(
                    f,
                    "Unsupported algorithm '{}'. Supported: {:?}",
                    algorithm, supported
                )
            }
            AccessDenied {
                required_level,
                current_level,
            } => {
                write!(
                    f,
                    "Access denied: requires level {}, current level {}",
                    required_level, current_level
                )
            }
            CorruptedData {
                expected_tag,
                actual_tag,
            } => {
                write!(
                    f,
                    "Data corruption detected: expected tag {}, got {}",
                    expected_tag, actual_tag
                )
            }
            InvalidIv {
                expected_len,
                actual_len,
            } => {
                write!(
                    f,
                    "Invalid IV: expected {} bytes, got {}",
                    expected_len, actual_len
                )
            }
            KeyDerivationFailed { reason } => write!(f, "Key derivation failed: {}", reason),
        }
    }
}

impl Error for EncryptionError {}

/// Access control errors
#[derive(Debug, Clone)]
pub enum AccessError {
    /// Permission denied for operation
    PermissionDenied {
        user: String,
        resource: String,
        action: String,
    },

    /// User is not the owner
    NotOwner {
        user: String,
        resource: String,
        owner: String,
    },

    /// Admin privileges required
    NotAdmin {
        user: String,
        required_action: String,
    },

    /// Access constraint violation
    ConstraintViolation {
        constraint_type: String,
        details: String,
    },

    /// Authentication failure
    AuthenticationFailed { method: String, reason: String },

    /// Authorization failure
    AuthorizationFailed { user: String, required_role: String },
}

impl fmt::Display for AccessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use AccessError::*;
        match self {
            PermissionDenied {
                user,
                resource,
                action,
            } => {
                write!(
                    f,
                    "Permission denied: user '{}' cannot {} on '{}'",
                    user, action, resource
                )
            }
            NotOwner {
                user,
                resource,
                owner,
            } => {
                write!(
                    f,
                    "Not owner: '{}' is not the owner of '{}' (owner: {})",
                    user, resource, owner
                )
            }
            NotAdmin {
                user,
                required_action,
            } => {
                write!(
                    f,
                    "Admin required: '{}' needs admin privileges for {}",
                    user, required_action
                )
            }
            ConstraintViolation {
                constraint_type,
                details,
            } => {
                write!(f, "Constraint violation ({}): {}", constraint_type, details)
            }
            AuthenticationFailed { method, reason } => {
                write!(f, "Authentication failed ({}): {}", method, reason)
            }
            AuthorizationFailed {
                user,
                required_role,
            } => {
                write!(
                    f,
                    "Authorization failed: '{}' requires role '{}'",
                    user, required_role
                )
            }
        }
    }
}

impl Error for AccessError {}

/// AI Provenance errors
#[derive(Debug, Clone)]
pub enum ProvenanceError {
    /// Provenance chain inconsistency
    InconsistentChain { expected_seq: u32, actual_seq: u32 },

    /// Invalid hash in provenance record
    InvalidHash {
        record_id: String,
        expected: String,
        actual: String,
    },

    /// AI command sequence error
    CommandSequenceError { command_id: String, details: String },

    /// Tracking level mismatch
    TrackingLevelMismatch { expected: String, actual: String },

    /// Privacy violation
    PrivacyViolation { field: String, reason: String },
}

impl fmt::Display for ProvenanceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ProvenanceError::*;
        match self {
            InconsistentChain {
                expected_seq,
                actual_seq,
            } => {
                write!(
                    f,
                    "Provenance chain broken: expected sequence {}, got {}",
                    expected_seq, actual_seq
                )
            }
            InvalidHash {
                record_id,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "Invalid hash in record {}: expected {}, got {}",
                    record_id, expected, actual
                )
            }
            CommandSequenceError {
                command_id,
                details,
            } => {
                write!(f, "Command sequence error in {}: {}", command_id, details)
            }
            TrackingLevelMismatch { expected, actual } => {
                write!(
                    f,
                    "Tracking level mismatch: expected {}, got {}",
                    expected, actual
                )
            }
            PrivacyViolation { field, reason } => {
                write!(f, "Privacy violation in field '{}': {}", field, reason)
            }
        }
    }
}

impl Error for ProvenanceError {}

/// Digital signature errors
#[derive(Debug, Clone)]
pub enum SignatureError {
    /// Invalid signature
    InvalidSignature { signer: String, reason: String },

    /// Certificate expired
    CertificateExpired { subject: String, expired_at: String },

    /// Untrusted certificate
    UntrustedCertificate { issuer: String, reason: String },

    /// Timestamp mismatch
    TimestampMismatch {
        expected: u64,
        actual: u64,
        tolerance_ms: u64,
    },

    /// Signature not found
    SignatureNotFound { signer_id: String },

    /// Multi-signature threshold not met
    ThresholdNotMet { required: u8, valid: u8 },

    /// Certificate chain validation failed
    ChainValidationFailed { details: String },
}

impl fmt::Display for SignatureError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use SignatureError::*;
        match self {
            InvalidSignature { signer, reason } => {
                write!(f, "Invalid signature from '{}': {}", signer, reason)
            }
            CertificateExpired {
                subject,
                expired_at,
            } => {
                write!(f, "Certificate for '{}' expired at {}", subject, expired_at)
            }
            UntrustedCertificate { issuer, reason } => {
                write!(f, "Untrusted certificate from '{}': {}", issuer, reason)
            }
            TimestampMismatch {
                expected,
                actual,
                tolerance_ms,
            } => {
                write!(
                    f,
                    "Timestamp mismatch: expected {}, got {} (tolerance: {}ms)",
                    expected, actual, tolerance_ms
                )
            }
            SignatureNotFound { signer_id } => {
                write!(f, "Signature not found for signer: {}", signer_id)
            }
            ThresholdNotMet { required, valid } => {
                write!(
                    f,
                    "Multi-signature threshold not met: {} required, {} valid",
                    required, valid
                )
            }
            ChainValidationFailed { details } => {
                write!(f, "Certificate chain validation failed: {}", details)
            }
        }
    }
}

impl Error for SignatureError {}

/// Key management errors
#[derive(Debug, Clone)]
pub enum KeyManagementError {
    /// Key generation failed
    KeyGenerationFailed { algorithm: String, reason: String },

    /// Key derivation failed
    KeyDerivationFailed { reason: String },

    /// Key rotation failed
    KeyRotationFailed { key_id: String, reason: String },

    /// Key not found
    KeyNotFound { key_id: String, key_type: String },

    /// Key escrow error
    EscrowError { operation: String, details: String },

    /// Hardware Security Module unavailable
    HsmUnavailable { hsm_id: String, reason: String },

    /// Key expired
    KeyExpired { key_id: String, expired_at: u64 },

    /// Invalid key format
    InvalidKeyFormat { expected: String, actual: String },
}

impl fmt::Display for KeyManagementError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use KeyManagementError::*;
        match self {
            KeyGenerationFailed { algorithm, reason } => {
                write!(f, "Key generation failed for {}: {}", algorithm, reason)
            }
            KeyDerivationFailed { reason } => {
                write!(f, "Key derivation failed: {}", reason)
            }
            KeyRotationFailed { key_id, reason } => {
                write!(f, "Key rotation failed for {}: {}", key_id, reason)
            }
            KeyNotFound { key_id, key_type } => {
                write!(f, "Key not found: {} (type: {})", key_id, key_type)
            }
            EscrowError { operation, details } => {
                write!(f, "Key escrow error during {}: {}", operation, details)
            }
            HsmUnavailable { hsm_id, reason } => {
                write!(f, "HSM '{}' unavailable: {}", hsm_id, reason)
            }
            KeyExpired { key_id, expired_at } => {
                write!(f, "Key '{}' expired at timestamp {}", key_id, expired_at)
            }
            InvalidKeyFormat { expected, actual } => {
                write!(
                    f,
                    "Invalid key format: expected {}, got {}",
                    expected, actual
                )
            }
        }
    }
}

impl Error for KeyManagementError {}

/// Audit log errors
#[derive(Debug, Clone)]
pub enum AuditError {
    /// Audit chain broken
    ChainBroken {
        break_index: usize,
        expected_hash: String,
    },

    /// Suspicious activity detected
    SuspiciousActivity {
        user: String,
        action: String,
        details: String,
    },

    /// Audit log full
    LogFull { max_entries: usize },

    /// Audit log tampered
    LogTampered { entry_index: usize, details: String },

    /// Log signature invalid
    InvalidLogSignature { signer: String },

    /// Log export failed
    ExportFailed { format: String, reason: String },
}

impl fmt::Display for AuditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use AuditError::*;
        match self {
            ChainBroken {
                break_index,
                expected_hash,
            } => {
                write!(
                    f,
                    "Audit chain broken at index {}: expected hash {}",
                    break_index, expected_hash
                )
            }
            SuspiciousActivity {
                user,
                action,
                details,
            } => {
                write!(
                    f,
                    "Suspicious activity by '{}' during {}: {}",
                    user, action, details
                )
            }
            LogFull { max_entries } => {
                write!(f, "Audit log full: maximum {} entries reached", max_entries)
            }
            LogTampered {
                entry_index,
                details,
            } => {
                write!(
                    f,
                    "Audit log tampered at entry {}: {}",
                    entry_index, details
                )
            }
            InvalidLogSignature { signer } => {
                write!(f, "Invalid audit log signature from '{}'", signer)
            }
            ExportFailed { format, reason } => {
                write!(
                    f,
                    "Audit log export failed (format: {}): {}",
                    format, reason
                )
            }
        }
    }
}

impl Error for AuditError {}

/// File format errors
#[derive(Debug, Clone)]
pub enum FormatError {
    /// Invalid magic bytes
    InvalidMagic { expected: Vec<u8>, actual: Vec<u8> },

    /// Invalid header
    InvalidHeader { field: String, reason: String },

    /// Invalid chunk
    InvalidChunk {
        chunk_type: String,
        offset: u64,
        reason: String,
    },

    /// CRC mismatch
    CrcMismatch {
        chunk: String,
        expected: u32,
        actual: u32,
    },

    /// Invalid chunk size
    InvalidChunkSize {
        chunk: String,
        size: u64,
        max_size: u64,
    },

    /// Missing required chunk
    MissingRequiredChunk { chunk_type: String },
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use FormatError::*;
        match self {
            InvalidMagic { expected, actual } => {
                write!(
                    f,
                    "Invalid magic bytes: expected {:?}, got {:?}",
                    expected, actual
                )
            }
            InvalidHeader { field, reason } => {
                write!(f, "Invalid header field '{}': {}", field, reason)
            }
            InvalidChunk {
                chunk_type,
                offset,
                reason,
            } => {
                write!(
                    f,
                    "Invalid chunk '{}' at offset {}: {}",
                    chunk_type, offset, reason
                )
            }
            CrcMismatch {
                chunk,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "CRC mismatch in chunk '{}': expected {:#x}, got {:#x}",
                    chunk, expected, actual
                )
            }
            InvalidChunkSize {
                chunk,
                size,
                max_size,
            } => {
                write!(
                    f,
                    "Invalid chunk size for '{}': {} bytes (max: {})",
                    chunk, size, max_size
                )
            }
            MissingRequiredChunk { chunk_type } => {
                write!(f, "Missing required chunk: {}", chunk_type)
            }
        }
    }
}

impl Error for FormatError {}

/// Version compatibility errors
#[derive(Debug, Clone)]
pub enum VersionError {
    /// Unsupported version
    UnsupportedVersion { major: u8, minor: u8, patch: u8 },

    /// Version too old
    VersionTooOld {
        file_version: String,
        min_supported: String,
    },

    /// Version too new
    VersionTooNew {
        file_version: String,
        max_supported: String,
    },

    /// Feature not available in version
    FeatureNotAvailable {
        feature: String,
        required_version: String,
    },

    /// Migration required
    MigrationRequired {
        from_version: String,
        to_version: String,
    },
}

impl fmt::Display for VersionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use VersionError::*;
        match self {
            UnsupportedVersion {
                major,
                minor,
                patch,
            } => {
                write!(f, "Unsupported version: {}.{}.{}", major, minor, patch)
            }
            VersionTooOld {
                file_version,
                min_supported,
            } => {
                write!(
                    f,
                    "Version {} too old (minimum supported: {})",
                    file_version, min_supported
                )
            }
            VersionTooNew {
                file_version,
                max_supported,
            } => {
                write!(
                    f,
                    "Version {} too new (maximum supported: {})",
                    file_version, max_supported
                )
            }
            FeatureNotAvailable {
                feature,
                required_version,
            } => {
                write!(
                    f,
                    "Feature '{}' requires version {}",
                    feature, required_version
                )
            }
            MigrationRequired {
                from_version,
                to_version,
            } => {
                write!(
                    f,
                    "Migration required from version {} to {}",
                    from_version, to_version
                )
            }
        }
    }
}

impl Error for VersionError {}

// Helper functions for error construction
impl RosFileError {
    /// Create an I/O error with additional context
    pub fn io_context(error: io::Error, context: &str) -> Self {
        RosFileError::Other {
            message: format!("I/O error in {}: {}", context, error),
            source: Some(Box::new(error)),
        }
    }

    /// Create a "not found" error
    pub fn not_found(resource: impl Into<String>, context: impl Into<String>) -> Self {
        RosFileError::NotFound {
            resource: resource.into(),
            context: context.into(),
        }
    }

    /// Create an "unsupported" error
    pub fn unsupported(operation: impl Into<String>, reason: impl Into<String>) -> Self {
        RosFileError::Unsupported {
            operation: operation.into(),
            reason: reason.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display() {
        let err = EncryptionError::MissingKey {
            key_id: "test-key-123".to_string(),
        };
        assert_eq!(err.to_string(), "Missing encryption key: test-key-123");

        let err = AccessError::PermissionDenied {
            user: "alice".to_string(),
            resource: "GEOM".to_string(),
            action: "modify".to_string(),
        };
        assert_eq!(
            err.to_string(),
            "Permission denied: user 'alice' cannot modify on 'GEOM'"
        );
    }

    #[test]
    fn test_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file not found");
        let roshera_err: RosFileError = io_err.into();
        assert!(matches!(roshera_err, RosFileError::Io(_)));
    }

    #[test]
    fn test_error_helpers() {
        let err = RosFileError::not_found("chunk", "loading file");
        match err {
            RosFileError::NotFound { resource, context } => {
                assert_eq!(resource, "chunk");
                assert_eq!(context, "loading file");
            }
            _ => panic!("Wrong error type"),
        }
    }

    #[test]
    fn test_error_source_chain() {
        let io_err = io::Error::new(io::ErrorKind::PermissionDenied, "access denied");
        let err = RosFileError::io_context(io_err, "opening file");

        assert!(err.source().is_some());
        assert!(err.to_string().contains("opening file"));
        assert!(err.to_string().contains("access denied"));
    }
}
