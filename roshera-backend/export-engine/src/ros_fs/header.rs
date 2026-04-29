// src/header.rs

//! .ros v3 File Header — Version, UUID, encryption/AI flags, CRC, etc.
//!
//! This module defines the Roshera v3 file header struct and parsing/writing logic.
//! Uses safe serialization instead of packed structs to avoid undefined behavior.

use crate::ros_fs::util::{crc32, current_time_ms, generate_uuid_v4};
use crate::ros_fs::{FormatError, Result, VersionError};
use byteorder::{BigEndian, ByteOrder, LittleEndian, ReadBytesExt, WriteBytesExt};
use std::io::{Read, Seek, SeekFrom, Write};

/// Magic string at start of every .ros file
pub const ROSHERA_MAGIC: &[u8; 8] = b"ROSHERA\0";

/// Current format version
pub const CURRENT_MAJOR_VERSION: u8 = 3;
pub const CURRENT_MINOR_VERSION: u8 = 0;
pub const CURRENT_PATCH_VERSION: u8 = 0;

/// Header size is always 128 bytes
pub const HEADER_SIZE: usize = 128;

/// Supported endianness
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endianness {
    Little = 1,
    Big = 2,
}

impl Endianness {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            1 => Ok(Endianness::Little),
            2 => Ok(Endianness::Big),
            _ => Err(FormatError::InvalidHeader {
                field: "endianness".to_string(),
                reason: format!("Invalid value: {}", value),
            }
            .into()),
        }
    }

    pub fn native() -> Self {
        if cfg!(target_endian = "little") {
            Endianness::Little
        } else {
            Endianness::Big
        }
    }
}

/// Feature flags as per v3 spec
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FeatureFlags(u64);

impl FeatureFlags {
    pub fn new() -> Self {
        FeatureFlags(0)
    }

    // Getters
    pub fn has_signature(&self) -> bool {
        self.0 & (1 << 0) != 0
    }
    pub fn has_thumbnails(&self) -> bool {
        self.0 & (1 << 1) != 0
    }
    pub fn extended_precision(&self) -> bool {
        self.0 & (1 << 2) != 0
    }
    pub fn animation_data(&self) -> bool {
        self.0 & (1 << 3) != 0
    }
    pub fn encrypted(&self) -> bool {
        self.0 & (1 << 4) != 0
    }
    pub fn ai_provenance(&self) -> bool {
        self.0 & (1 << 5) != 0
    }
    pub fn blockchain(&self) -> bool {
        self.0 & (1 << 6) != 0
    }
    pub fn multisig(&self) -> bool {
        self.0 & (1 << 7) != 0
    }
    pub fn hsm_signed(&self) -> bool {
        self.0 & (1 << 8) != 0
    }
    pub fn timestamped(&self) -> bool {
        self.0 & (1 << 9) != 0
    }
    pub fn redacted(&self) -> bool {
        self.0 & (1 << 10) != 0
    }
    pub fn collaborative(&self) -> bool {
        self.0 & (1 << 11) != 0
    }

    // Setters (builder pattern)
    pub fn with_signature(mut self) -> Self {
        self.0 |= 1 << 0;
        self
    }
    pub fn with_thumbnails(mut self) -> Self {
        self.0 |= 1 << 1;
        self
    }
    pub fn with_extended_precision(mut self) -> Self {
        self.0 |= 1 << 2;
        self
    }
    pub fn with_animation_data(mut self) -> Self {
        self.0 |= 1 << 3;
        self
    }
    pub fn with_encryption(mut self) -> Self {
        self.0 |= 1 << 4;
        self
    }
    pub fn with_ai_provenance(mut self) -> Self {
        self.0 |= 1 << 5;
        self
    }
    pub fn with_blockchain(mut self) -> Self {
        self.0 |= 1 << 6;
        self
    }
    pub fn with_multisig(mut self) -> Self {
        self.0 |= 1 << 7;
        self
    }
    pub fn with_hsm_signed(mut self) -> Self {
        self.0 |= 1 << 8;
        self
    }
    pub fn with_timestamped(mut self) -> Self {
        self.0 |= 1 << 9;
        self
    }
    pub fn with_redacted(mut self) -> Self {
        self.0 |= 1 << 10;
        self
    }
    pub fn with_collaborative(mut self) -> Self {
        self.0 |= 1 << 11;
        self
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u64> for FeatureFlags {
    fn from(bits: u64) -> Self {
        FeatureFlags(bits)
    }
}

/// Top-level file header for .ros v3 (128 bytes)
#[derive(Debug, Clone)]
pub struct FileHeader {
    // Version info (16 bytes)
    pub magic: [u8; 8],         // "ROSHERA\0"
    pub major_version: u8,      // e.g. 3
    pub minor_version: u8,      // e.g. 0
    pub patch_version: u8,      // e.g. 0
    pub endianness: Endianness, // 1=little, 2=big
    pub header_crc32: u32,      // CRC32 of bytes 0..123

    // Metadata (32 bytes)
    pub file_size: u64,
    pub creation_time: u64, // Unix ms
    pub file_uuid: [u8; 16],

    // Chunk index location (16 bytes)
    pub index_offset: u64,
    pub index_entry_count: u32,
    pub index_entry_size: u32, // Always 96

    // Security features (32 bytes)
    pub encryption_algo: u8, // 0=none, 1=AES, 2=ChaCha
    pub kdf_algo: u8,        // 0=none, 1=PBKDF2, 2=Argon2
    pub signature_algo: u8,  // 0=none, 1=Ed25519, 2=ECDSA
    pub ai_tracking: u8,     // 0=none, 1=basic, 2=detailed, 3=forensic
    pub kdf_iterations: u32,
    pub kdf_salt: [u8; 16],
    pub file_iv: [u8; 8],

    // Feature flags (16 bytes)
    pub feature_flags: FeatureFlags,
    pub reserved: [u8; 8], // Must be zero

    // AI provenance hint (16 bytes)
    pub ai_command_count: u64,
    pub ai_chunk_offset: u64,
}

impl FileHeader {
    /// Create a new header with defaults
    pub fn new() -> Self {
        FileHeader {
            magic: *ROSHERA_MAGIC,
            major_version: CURRENT_MAJOR_VERSION,
            minor_version: CURRENT_MINOR_VERSION,
            patch_version: CURRENT_PATCH_VERSION,
            endianness: Endianness::native(),
            header_crc32: 0, // Calculated on write

            file_size: 0,
            creation_time: current_time_ms(),
            file_uuid: generate_uuid_v4(),

            index_offset: 0,
            index_entry_count: 0,
            index_entry_size: 96,

            encryption_algo: 0,
            kdf_algo: 0,
            signature_algo: 0,
            ai_tracking: 0,
            kdf_iterations: 0,
            kdf_salt: [0; 16],
            file_iv: [0; 8],

            feature_flags: FeatureFlags::new(),
            reserved: [0; 8],

            ai_command_count: 0,
            ai_chunk_offset: 0,
        }
    }

    /// Create a builder for constructing headers
    pub fn builder() -> FileHeaderBuilder {
        FileHeaderBuilder::new()
    }

    /// Read header from file (seeks to start, reads 128 bytes)
    pub fn read_from<R: Read + Seek>(reader: &mut R) -> Result<Self> {
        reader.seek(SeekFrom::Start(0))?;

        // Read the header bytes
        let mut header_bytes = vec![0u8; HEADER_SIZE];
        reader.read_exact(&mut header_bytes)?;

        // Parse magic and determine endianness
        let magic =
            <[u8; 8]>::try_from(&header_bytes[0..8]).map_err(|_| FormatError::InvalidHeader {
                field: "magic".to_string(),
                reason: "Failed to read magic bytes".to_string(),
            })?;

        if magic != *ROSHERA_MAGIC {
            return Err(FormatError::InvalidMagic {
                expected: ROSHERA_MAGIC.to_vec(),
                actual: magic.to_vec(),
            }
            .into());
        }

        // Determine endianness from byte 11
        let endianness = Endianness::from_u8(header_bytes[11])?;

        // Parse the rest based on endianness
        let mut cursor = std::io::Cursor::new(&header_bytes);
        cursor.seek(SeekFrom::Start(8))?;

        let header = match endianness {
            Endianness::Little => Self::parse_with_endianness::<LittleEndian>(&mut cursor, magic)?,
            Endianness::Big => Self::parse_with_endianness::<BigEndian>(&mut cursor, magic)?,
        };

        // Verify CRC (exclude the CRC field itself)
        let computed_crc = crc32(&header_bytes[0..12]);
        let stored_crc_bytes = &header_bytes[12..16];
        let stored_crc = match endianness {
            Endianness::Little => LittleEndian::read_u32(stored_crc_bytes),
            Endianness::Big => BigEndian::read_u32(stored_crc_bytes),
        };

        if computed_crc != stored_crc {
            return Err(FormatError::CrcMismatch {
                chunk: "header".to_string(),
                expected: stored_crc,
                actual: computed_crc,
            }
            .into());
        }

        Ok(header)
    }

    /// Parse header with specific endianness
    fn parse_with_endianness<E: ByteOrder>(
        cursor: &mut std::io::Cursor<&Vec<u8>>,
        magic: [u8; 8],
    ) -> Result<Self> {
        let major_version = cursor.read_u8()?;
        let minor_version = cursor.read_u8()?;
        let patch_version = cursor.read_u8()?;
        let endianness = Endianness::from_u8(cursor.read_u8()?)?;
        let header_crc32 = cursor.read_u32::<E>()?;

        let file_size = cursor.read_u64::<E>()?;
        let creation_time = cursor.read_u64::<E>()?;
        let mut file_uuid = [0u8; 16];
        cursor.read_exact(&mut file_uuid)?;

        let index_offset = cursor.read_u64::<E>()?;
        let index_entry_count = cursor.read_u32::<E>()?;
        let index_entry_size = cursor.read_u32::<E>()?;

        let encryption_algo = cursor.read_u8()?;
        let kdf_algo = cursor.read_u8()?;
        let signature_algo = cursor.read_u8()?;
        let ai_tracking = cursor.read_u8()?;
        let kdf_iterations = cursor.read_u32::<E>()?;
        let mut kdf_salt = [0u8; 16];
        cursor.read_exact(&mut kdf_salt)?;
        let mut file_iv = [0u8; 8];
        cursor.read_exact(&mut file_iv)?;

        let feature_flags = FeatureFlags(cursor.read_u64::<E>()?);
        let mut reserved = [0u8; 8];
        cursor.read_exact(&mut reserved)?;

        let ai_command_count = cursor.read_u64::<E>()?;
        let ai_chunk_offset = cursor.read_u64::<E>()?;

        Ok(FileHeader {
            magic,
            major_version,
            minor_version,
            patch_version,
            endianness,
            header_crc32,
            file_size,
            creation_time,
            file_uuid,
            index_offset,
            index_entry_count,
            index_entry_size,
            encryption_algo,
            kdf_algo,
            signature_algo,
            ai_tracking,
            kdf_iterations,
            kdf_salt,
            file_iv,
            feature_flags,
            reserved,
            ai_command_count,
            ai_chunk_offset,
        })
    }

    /// Write header to file (seeks to start, writes 128 bytes)
    pub fn write_to<W: Write + Seek>(&mut self, writer: &mut W) -> Result<()> {
        // Ensure we're at the start
        writer.seek(SeekFrom::Start(0))?;

        // Prepare header bytes
        let mut header_bytes = vec![0u8; HEADER_SIZE];
        let mut cursor = std::io::Cursor::new(&mut header_bytes);

        // Write based on endianness
        match self.endianness {
            Endianness::Little => self.serialize_with_endianness::<LittleEndian>(&mut cursor)?,
            Endianness::Big => self.serialize_with_endianness::<BigEndian>(&mut cursor)?,
        }

        // Calculate and update CRC
        let crc = crc32(&header_bytes[0..12]);
        match self.endianness {
            Endianness::Little => LittleEndian::write_u32(&mut header_bytes[12..16], crc),
            Endianness::Big => BigEndian::write_u32(&mut header_bytes[12..16], crc),
        }
        self.header_crc32 = crc;

        // Write to output
        writer.write_all(&header_bytes)?;
        writer.flush()?;

        Ok(())
    }

    /// Serialize header with specific endianness
    fn serialize_with_endianness<E: ByteOrder>(
        &self,
        cursor: &mut std::io::Cursor<&mut Vec<u8>>,
    ) -> Result<()> {
        cursor.write_all(&self.magic)?;
        cursor.write_u8(self.major_version)?;
        cursor.write_u8(self.minor_version)?;
        cursor.write_u8(self.patch_version)?;
        cursor.write_u8(self.endianness as u8)?;
        cursor.write_u32::<E>(self.header_crc32)?;

        cursor.write_u64::<E>(self.file_size)?;
        cursor.write_u64::<E>(self.creation_time)?;
        cursor.write_all(&self.file_uuid)?;

        cursor.write_u64::<E>(self.index_offset)?;
        cursor.write_u32::<E>(self.index_entry_count)?;
        cursor.write_u32::<E>(self.index_entry_size)?;

        cursor.write_u8(self.encryption_algo)?;
        cursor.write_u8(self.kdf_algo)?;
        cursor.write_u8(self.signature_algo)?;
        cursor.write_u8(self.ai_tracking)?;
        cursor.write_u32::<E>(self.kdf_iterations)?;
        cursor.write_all(&self.kdf_salt)?;
        cursor.write_all(&self.file_iv)?;

        cursor.write_u64::<E>(self.feature_flags.as_u64())?;
        cursor.write_all(&self.reserved)?;

        cursor.write_u64::<E>(self.ai_command_count)?;
        cursor.write_u64::<E>(self.ai_chunk_offset)?;

        Ok(())
    }

    /// Check if this header is supported by the current implementation
    pub fn is_supported(&self) -> Result<()> {
        if self.magic != *ROSHERA_MAGIC {
            return Err(FormatError::InvalidMagic {
                expected: ROSHERA_MAGIC.to_vec(),
                actual: self.magic.to_vec(),
            }
            .into());
        }

        if self.major_version > CURRENT_MAJOR_VERSION {
            return Err(VersionError::VersionTooNew {
                file_version: format!(
                    "{}.{}.{}",
                    self.major_version, self.minor_version, self.patch_version
                ),
                max_supported: format!(
                    "{}.{}.{}",
                    CURRENT_MAJOR_VERSION, CURRENT_MINOR_VERSION, CURRENT_PATCH_VERSION
                ),
            }
            .into());
        }

        if self.major_version < 3 {
            return Err(VersionError::VersionTooOld {
                file_version: format!(
                    "{}.{}.{}",
                    self.major_version, self.minor_version, self.patch_version
                ),
                min_supported: "3.0.0".to_string(),
            }
            .into());
        }

        Ok(())
    }

    /// Validate that all header fields are consistent
    pub fn validate(&self) -> Result<()> {
        // Check version
        self.is_supported()?;

        // Validate reserved bytes are zero
        if !self.reserved.iter().all(|&b| b == 0) {
            return Err(FormatError::InvalidHeader {
                field: "reserved".to_string(),
                reason: "Reserved bytes must be zero".to_string(),
            }
            .into());
        }

        // Validate index entry size
        if self.index_entry_size != 96 {
            return Err(FormatError::InvalidHeader {
                field: "index_entry_size".to_string(),
                reason: format!("Must be 96, got {}", self.index_entry_size),
            }
            .into());
        }

        // Validate encryption settings
        if self.encryption_algo > 2 {
            return Err(FormatError::InvalidHeader {
                field: "encryption_algo".to_string(),
                reason: format!("Invalid algorithm ID: {}", self.encryption_algo),
            }
            .into());
        }

        // If encrypted, ensure we have KDF settings
        if self.encryption_algo != 0 && self.kdf_algo == 0 {
            return Err(FormatError::InvalidHeader {
                field: "kdf_algo".to_string(),
                reason: "KDF required when encryption is enabled".to_string(),
            }
            .into());
        }

        Ok(())
    }

    /// Get version string
    pub fn version_string(&self) -> String {
        format!(
            "{}.{}.{}",
            self.major_version, self.minor_version, self.patch_version
        )
    }
}

/// Builder for FileHeader
pub struct FileHeaderBuilder {
    header: FileHeader,
}

impl FileHeaderBuilder {
    pub fn new() -> Self {
        FileHeaderBuilder {
            header: FileHeader::new(),
        }
    }

    pub fn with_encryption(
        mut self,
        algo: u8,
        kdf_algo: u8,
        iterations: u32,
        salt: [u8; 16],
        iv: [u8; 8],
    ) -> Self {
        self.header.encryption_algo = algo;
        self.header.kdf_algo = kdf_algo;
        self.header.kdf_iterations = iterations;
        self.header.kdf_salt = salt;
        self.header.file_iv = iv;
        self.header.feature_flags = self.header.feature_flags.with_encryption();
        self
    }

    pub fn with_ai_tracking(mut self, level: u8) -> Self {
        self.header.ai_tracking = level;
        self.header.feature_flags = self.header.feature_flags.with_ai_provenance();
        self
    }

    pub fn with_signature(mut self, algo: u8) -> Self {
        self.header.signature_algo = algo;
        self.header.feature_flags = self.header.feature_flags.with_signature();
        self
    }

    pub fn with_file_size(mut self, size: u64) -> Self {
        self.header.file_size = size;
        self
    }

    pub fn with_index_info(mut self, offset: u64, count: u32) -> Self {
        self.header.index_offset = offset;
        self.header.index_entry_count = count;
        self
    }

    pub fn with_feature_flags(mut self, flags: FeatureFlags) -> Self {
        self.header.feature_flags = flags;
        self
    }

    pub fn build(self) -> FileHeader {
        self.header
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ros_fs::RosFileError;
    use std::io::Cursor;

    #[test]
    fn test_header_roundtrip() {
        let original = FileHeader::builder()
            .with_encryption(1, 2, 10000, [1; 16], [2; 8])
            .with_ai_tracking(2)
            .with_signature(1)
            .with_file_size(1024 * 1024)
            .with_index_info(2048, 10)
            .build();

        // Write to buffer
        let mut buffer = Cursor::new(vec![0u8; HEADER_SIZE]);
        let mut header = original.clone();
        header.write_to(&mut buffer).unwrap();

        // Read back
        buffer.seek(SeekFrom::Start(0)).unwrap();
        let read_header = FileHeader::read_from(&mut buffer).unwrap();

        // Compare fields
        assert_eq!(read_header.magic, original.magic);
        assert_eq!(read_header.major_version, original.major_version);
        assert_eq!(read_header.encryption_algo, original.encryption_algo);
        assert_eq!(read_header.file_size, original.file_size);
        assert_eq!(
            read_header.feature_flags.as_u64(),
            original.feature_flags.as_u64()
        );
    }

    #[test]
    fn test_feature_flags() {
        let flags = FeatureFlags::new()
            .with_encryption()
            .with_ai_provenance()
            .with_signature();

        assert!(flags.encrypted());
        assert!(flags.ai_provenance());
        assert!(flags.has_signature());
        assert!(!flags.blockchain());
    }

    #[test]
    fn test_version_validation() {
        let mut header = FileHeader::new();
        assert!(header.validate().is_ok());

        header.major_version = 2;
        assert!(matches!(
            header.validate(),
            Err(RosFileError::Version(VersionError::VersionTooOld { .. }))
        ));

        header.major_version = 4;
        assert!(matches!(
            header.validate(),
            Err(RosFileError::Version(VersionError::VersionTooNew { .. }))
        ));
    }

    #[test]
    fn test_invalid_magic() {
        let mut buffer = vec![0u8; HEADER_SIZE];
        buffer[0..8].copy_from_slice(b"INVALID\0");

        let mut cursor = Cursor::new(buffer);
        let result = FileHeader::read_from(&mut cursor);
        assert!(matches!(
            result,
            Err(RosFileError::Format(FormatError::InvalidMagic { .. }))
        ));
    }

    #[test]
    fn test_endianness_handling() {
        let header = FileHeader::builder()
            .with_file_size(0x1234567890ABCDEF)
            .build();

        // Test both endianness
        for endianness in [Endianness::Little, Endianness::Big] {
            let mut h = header.clone();
            h.endianness = endianness;

            let mut buffer = Cursor::new(vec![0u8; HEADER_SIZE]);
            h.write_to(&mut buffer).unwrap();

            buffer.seek(SeekFrom::Start(0)).unwrap();
            let read_header = FileHeader::read_from(&mut buffer).unwrap();

            assert_eq!(read_header.file_size, header.file_size);
            assert_eq!(read_header.endianness, endianness);
        }
    }
}
