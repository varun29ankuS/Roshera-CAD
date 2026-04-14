// src/compat.rs

//! Compatibility Layer for .ros v2/v3
//!
//! - Detects, opens, and upgrades legacy .ros v2 files
//! - Migrates v2 files to v3 with new features (AI tracking, encryption, etc.)
//! - All new files use v3 by default

use crate::ros_fs::aipr::{AICommandTracker, PrivacySettings, TrackingLevel};
use crate::ros_fs::chunk::{Chunk, ChunkType};
use crate::ros_fs::encryption::{ChunkEncryptor, EncryptionAlgorithm};
use crate::ros_fs::keys::{KeyManager, SoftwareKeyManager};
use crate::ros_fs::{Result, RosFileError};
use byteorder::ReadBytesExt;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Supported .ros file version
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosVersion {
    V2,
    V3,
}

impl RosVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            RosVersion::V2 => "2.0",
            RosVersion::V3 => "3.0",
        }
    }
}

/// Checks the magic/version and returns detected format
pub fn detect_ros_version<R: Read + Seek>(mut reader: R) -> Result<RosVersion> {
    let mut magic = [0u8; 8];
    reader.seek(SeekFrom::Start(0))?;
    reader.read_exact(&mut magic)?;

    if &magic != b"ROSHERA\0" {
        return Err(RosFileError::Format(
            crate::ros_fs::FormatError::InvalidMagic {
                expected: b"ROSHERA\0".to_vec(),
                actual: magic.to_vec(),
            },
        ));
    }

    let major = reader.read_u8()?;
    let minor = reader.read_u8()?;
    let _patch = reader.read_u8()?;

    match (major, minor) {
        (0, 2) => Ok(RosVersion::V2),
        (3, _) => Ok(RosVersion::V3),
        _ => Err(RosFileError::Version(
            crate::ros_fs::VersionError::UnsupportedVersion {
                major,
                minor,
                patch: 0,
            },
        )),
    }
}

/// File compatibility handle
pub enum RosFileCompat {
    V2(File), // read-only, limited features
    V3(File), // full support
}

impl RosFileCompat {
    /// Get the file version
    pub fn version(&self) -> RosVersion {
        match self {
            RosFileCompat::V2(_) => RosVersion::V2,
            RosFileCompat::V3(_) => RosVersion::V3,
        }
    }

    /// Check if file supports a feature
    pub fn supports_encryption(&self) -> bool {
        matches!(self, RosFileCompat::V3(_))
    }

    pub fn supports_ai_tracking(&self) -> bool {
        matches!(self, RosFileCompat::V3(_))
    }
}

/// Opens either a v2 or v3 file
pub fn open_ros_file<P: AsRef<Path>>(path: P) -> Result<RosFileCompat> {
    let mut file = File::open(path)?;
    let version = detect_ros_version(&mut file)?;

    match version {
        RosVersion::V3 => Ok(RosFileCompat::V3(file)),
        RosVersion::V2 => Ok(RosFileCompat::V2(file)),
    }
}

/// Migration options for v2 to v3
pub struct MigrationOptions {
    pub enable_ai_tracking: bool,
    pub tracking_level: TrackingLevel,
    pub enable_encryption: bool,
    pub encryption_algorithm: EncryptionAlgorithm,
    pub password: Option<String>,
    pub compress_chunks: bool,
}

impl Default for MigrationOptions {
    fn default() -> Self {
        MigrationOptions {
            enable_ai_tracking: false,
            tracking_level: TrackingLevel::Basic,
            enable_encryption: false,
            encryption_algorithm: EncryptionAlgorithm::AES256GCM,
            password: None,
            compress_chunks: false,
        }
    }
}

/// Migrates a v2 file to v3 with specified options
pub fn migrate_v2_to_v3<P: AsRef<Path>>(
    v2_path: P,
    v3_path: P,
    options: MigrationOptions,
) -> Result<()> {
    // Open and verify v2 file
    let mut v2_file = File::open(v2_path)?;
    let version = detect_ros_version(&mut v2_file)?;
    if version != RosVersion::V2 {
        return Err(RosFileError::Version(
            crate::ros_fs::VersionError::MigrationRequired {
                from_version: version.as_str().to_string(),
                to_version: "3.0".to_string(),
            },
        ));
    }

    // Load v2 chunks (placeholder - implement actual v2 parser)
    let v2_chunks = load_v2_chunks(&mut v2_file)?;

    // Initialize AI tracking if requested
    let ai_tracker = if options.enable_ai_tracking {
        Some(AICommandTracker::new(
            options.tracking_level,
            PrivacySettings::default(),
            None,
        ))
    } else {
        None
    };

    // Set up encryption if requested
    let (key_set, file_iv): (Option<crate::ros_fs::keys::KeySet>, Option<[u8; 8]>) =
        if options.enable_encryption {
            let password = options
                .password
                .as_ref()
                .ok_or_else(|| RosFileError::Other {
                    message: "Password required for encryption".to_string(),
                    source: None,
                })?;

            let salt = crate::ros_fs::util::random_16();
            let key_manager = SoftwareKeyManager::default();
            let keys = key_manager.generate_key_set(password, &salt)?;
            let iv: [u8; 8] = crate::ros_fs::util::random_bytes(8).try_into().unwrap();

            (Some(keys), Some(iv))
        } else {
            (None, None)
        };

    // Process chunks
    let mut v3_chunks = Vec::new();

    for mut chunk in v2_chunks {
        // Encrypt sensitive chunks if encryption is enabled
        if let (Some(keys), Some(iv)) = (&key_set, &file_iv) {
            if should_encrypt_chunk(chunk.chunk_type()) {
                let encryptor =
                    ChunkEncryptor::new(options.encryption_algorithm, keys.clone(), *iv);

                let encrypted_data = encryptor.encrypt_chunk(
                    &chunk.index.chunk_type,
                    &chunk.data,
                    v3_chunks.len() as u32,
                    None,
                )?;

                chunk.data = encrypted_data;
                chunk.index.encrypted = true;
                chunk.index.enc_algo = options.encryption_algorithm.as_id();
                chunk.index.key_id = keys.file_key.id;

                // Update CRC for encrypted data
                chunk.update_crc();
            }
        }

        v3_chunks.push(chunk);
    }

    // Add AI tracking chunk if enabled
    if let Some(tracker) = ai_tracker {
        // Record migration as first AI command
        let mut tracker_mut = tracker;
        tracker_mut.track_command(
            crate::ros_fs::aipr::CommandType::Create,
            [0u8; 32], // Migration tool ID
            1,
            "Migrated from v2 to v3",
            "File migration completed",
            &["file".to_string()],
            1.0,
            100,
            None,
        );

        let aipr_chunk = Chunk::new(ChunkType::AIPR, tracker_mut.serialize());
        v3_chunks.push(aipr_chunk);
    }

    // Write v3 file
    write_v3_file(v3_path, v3_chunks, key_set, file_iv)?;

    Ok(())
}

/// Determines if a chunk should be encrypted
fn should_encrypt_chunk(chunk_type: ChunkType) -> bool {
    chunk_type.should_encrypt()
}

/// Load chunks from v2 file (placeholder implementation)
fn load_v2_chunks(_file: &mut File) -> Result<Vec<Chunk>> {
    // TODO: Implement actual v2 chunk loading
    // This would involve:
    // 1. Reading v2 header
    // 2. Reading v2 chunk table
    // 3. Loading each chunk with v2 format
    // 4. Converting to v3 chunk format

    Ok(Vec::new())
}

/// Write a complete v3 file
fn write_v3_file<P: AsRef<Path>>(
    path: P,
    chunks: Vec<Chunk>,
    key_set: Option<crate::ros_fs::keys::KeySet>,
    file_iv: Option<[u8; 8]>,
) -> Result<()> {
    use crate::ros_fs::header::FileHeader;
    use std::fs::OpenOptions;
    use std::io::Write;

    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    // Build header
    let mut builder = FileHeader::builder()
        .with_file_size(0) // Will be updated after writing chunks
        .with_index_info(
            128, // Index starts after header
            chunks.len() as u32,
        );

    // Add encryption info if applicable
    if let (Some(keys), Some(iv)) = (key_set, file_iv) {
        builder = builder.with_encryption(
            1,              // AES-256-GCM
            2,              // Argon2
            10_000,         // iterations
            keys.master.id, // Using master key ID as salt (in production, use proper salt)
            iv,
        );
    }

    let mut header = builder.build();

    // Write header (placeholder)
    header.write_to(&mut file)?;

    // Write chunk index and data
    // TODO: Implement actual chunk writing

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_detection() {
        use std::io::Cursor;

        // Test v3 detection
        let mut v3_data = Vec::new();
        v3_data.extend_from_slice(b"ROSHERA\0");
        v3_data.push(3); // major
        v3_data.push(0); // minor
        v3_data.push(0); // patch

        let cursor = Cursor::new(v3_data);
        let version = detect_ros_version(cursor).unwrap();
        assert_eq!(version, RosVersion::V3);

        // Test v2 detection
        let mut v2_data = Vec::new();
        v2_data.extend_from_slice(b"ROSHERA\0");
        v2_data.push(0); // major
        v2_data.push(2); // minor
        v2_data.push(0); // patch

        let cursor = Cursor::new(v2_data);
        let version = detect_ros_version(cursor).unwrap();
        assert_eq!(version, RosVersion::V2);

        // Test invalid magic
        let bad_data = b"INVALID\0\x03\x00\x00";
        let cursor = Cursor::new(bad_data);
        assert!(detect_ros_version(cursor).is_err());
    }

    #[test]
    fn test_migration_options() {
        let opts = MigrationOptions::default();
        assert!(!opts.enable_ai_tracking);
        assert!(!opts.enable_encryption);
        assert_eq!(opts.encryption_algorithm, EncryptionAlgorithm::AES256GCM);
    }
}
