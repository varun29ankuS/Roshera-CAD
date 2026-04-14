// src/chunk.rs

//! .ros v3 Chunk Table & Chunk Abstractions
//!
//! - ChunkIndexEntry: 96-byte per-chunk index (offsets, flags, crypto, ACL)
//! - Chunk: in-memory representation (type, version, data, metadata)
//! - FourCC: chunk type codes
//! - Safe serialization without packed structs

use crate::ros_fs::util::{crc32, is_all_zeros};
use crate::ros_fs::{FormatError, Result, RosFileError};
use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use std::fmt;
use std::io::{Read, Seek, SeekFrom, Write};

/// Four-character chunk type code (e.g. 'GEOM', 'AIPR')
pub type FourCC = [u8; 4];

/// Chunk index entry size (fixed at 96 bytes)
pub const CHUNK_INDEX_ENTRY_SIZE: usize = 96;

/// Maximum chunk size (1GB)
pub const MAX_CHUNK_SIZE: u64 = 1024 * 1024 * 1024;

/// Known chunk types (.ros v3, see spec §5)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ChunkType {
    META,
    GEOM,
    TOPO,
    FEAT,
    CONS,
    AIPR,
    KEYS,
    BCHN,
    ACLS,
    SIGN,
    CUSTOM(FourCC),
}

impl ChunkType {
    pub fn from_fourcc(code: FourCC) -> Self {
        match &code {
            b"META" => ChunkType::META,
            b"GEOM" => ChunkType::GEOM,
            b"TOPO" => ChunkType::TOPO,
            b"FEAT" => ChunkType::FEAT,
            b"CONS" => ChunkType::CONS,
            b"AIPR" => ChunkType::AIPR,
            b"KEYS" => ChunkType::KEYS,
            b"BCHN" => ChunkType::BCHN,
            b"ACLS" => ChunkType::ACLS,
            b"SIGN" => ChunkType::SIGN,
            other => ChunkType::CUSTOM(*other),
        }
    }

    pub fn as_fourcc(&self) -> FourCC {
        match self {
            ChunkType::META => *b"META",
            ChunkType::GEOM => *b"GEOM",
            ChunkType::TOPO => *b"TOPO",
            ChunkType::FEAT => *b"FEAT",
            ChunkType::CONS => *b"CONS",
            ChunkType::AIPR => *b"AIPR",
            ChunkType::KEYS => *b"KEYS",
            ChunkType::BCHN => *b"BCHN",
            ChunkType::ACLS => *b"ACLS",
            ChunkType::SIGN => *b"SIGN",
            ChunkType::CUSTOM(c) => *c,
        }
    }

    pub fn as_str(&self) -> String {
        let fourcc = self.as_fourcc();
        String::from_utf8_lossy(&fourcc).to_string()
    }

    /// Check if this is a required chunk type
    pub fn is_required(&self) -> bool {
        matches!(self, ChunkType::META | ChunkType::GEOM)
    }

    /// Check if this chunk type should be encrypted by default
    pub fn should_encrypt(&self) -> bool {
        matches!(
            self,
            ChunkType::GEOM | ChunkType::TOPO | ChunkType::FEAT | ChunkType::AIPR
        )
    }
}

impl fmt::Display for ChunkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Compression algorithms
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionAlgorithm {
    None = 0,
    Inherit = 1,
    Zstd = 2,
    Lzma = 3,
    Brotli = 4,
}

impl CompressionAlgorithm {
    pub fn from_u8(value: u8) -> Result<Self> {
        match value {
            0 => Ok(CompressionAlgorithm::None),
            1 => Ok(CompressionAlgorithm::Inherit),
            2 => Ok(CompressionAlgorithm::Zstd),
            3 => Ok(CompressionAlgorithm::Lzma),
            4 => Ok(CompressionAlgorithm::Brotli),
            _ => Err(FormatError::InvalidChunk {
                chunk_type: "unknown".to_string(),
                offset: 0,
                reason: format!("Invalid compression algorithm: {}", value),
            }
            .into()),
        }
    }
}

/// v3 Chunk index entry: 96 bytes (see spec §4)
#[derive(Debug, Clone)]
pub struct ChunkIndexEntry {
    pub chunk_type: FourCC,     // FourCC
    pub version: u32,           // Chunk format version
    pub offset: u64,            // Byte offset in file
    pub uncompressed_size: u64, // Logical size
    pub compressed_size: u64,   // On disk (0 = uncompressed)
    pub crc32: u32,             // CRC32 of compressed data
    pub flags: u32,             // Chunk-specific flags

    // Compression (8 bytes)
    pub compression: CompressionAlgorithm,
    pub comp_level: u8,         // Compression level
    pub reserved_comp: [u8; 6], // Reserved

    // Encryption (32 bytes)
    pub encrypted: bool,
    pub enc_algo: u8,       // Algorithm override
    pub key_id: [u8; 16],   // Key identifier
    pub chunk_iv: [u8; 12], // Initialization vector
    pub auth_tag: [u8; 2],  // Auth tag preview

    // Access control (8 bytes)
    pub access_level: u32, // Required access level
    pub owner_id: u32,     // Owner ID

    // Reserved (8 bytes)
    pub reserved: [u8; 8], // Must be zero
}

impl ChunkIndexEntry {
    /// Create a new chunk index entry with defaults
    pub fn new(chunk_type: FourCC) -> Self {
        ChunkIndexEntry {
            chunk_type,
            version: 1,
            offset: 0,
            uncompressed_size: 0,
            compressed_size: 0,
            crc32: 0,
            flags: 0,
            compression: CompressionAlgorithm::None,
            comp_level: 0,
            reserved_comp: [0; 6],
            encrypted: false,
            enc_algo: 0,
            key_id: [0; 16],
            chunk_iv: [0; 12],
            auth_tag: [0; 2],
            access_level: 0,
            owner_id: 0,
            reserved: [0; 8],
        }
    }

    /// Read 96-byte chunk index entry from reader
    pub fn read_from<R: Read>(reader: &mut R) -> Result<Self> {
        let mut chunk_type = [0u8; 4];
        reader.read_exact(&mut chunk_type)?;

        let version = reader.read_u32::<LittleEndian>()?;
        let offset = reader.read_u64::<LittleEndian>()?;
        let uncompressed_size = reader.read_u64::<LittleEndian>()?;
        let compressed_size = reader.read_u64::<LittleEndian>()?;
        let crc32 = reader.read_u32::<LittleEndian>()?;
        let flags = reader.read_u32::<LittleEndian>()?;

        let compression = CompressionAlgorithm::from_u8(reader.read_u8()?)?;
        let comp_level = reader.read_u8()?;
        let mut reserved_comp = [0u8; 6];
        reader.read_exact(&mut reserved_comp)?;

        let encrypted = reader.read_u8()? != 0;
        let enc_algo = reader.read_u8()?;
        let mut key_id = [0u8; 16];
        reader.read_exact(&mut key_id)?;
        let mut chunk_iv = [0u8; 12];
        reader.read_exact(&mut chunk_iv)?;
        let mut auth_tag = [0u8; 2];
        reader.read_exact(&mut auth_tag)?;

        let access_level = reader.read_u32::<LittleEndian>()?;
        let owner_id = reader.read_u32::<LittleEndian>()?;

        let mut reserved = [0u8; 8];
        reader.read_exact(&mut reserved)?;

        let entry = ChunkIndexEntry {
            chunk_type,
            version,
            offset,
            uncompressed_size,
            compressed_size,
            crc32,
            flags,
            compression,
            comp_level,
            reserved_comp,
            encrypted,
            enc_algo,
            key_id,
            chunk_iv,
            auth_tag,
            access_level,
            owner_id,
            reserved,
        };

        entry.validate()?;
        Ok(entry)
    }

    /// Write chunk index entry (96 bytes)
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        writer.write_all(&self.chunk_type)?;
        writer.write_u32::<LittleEndian>(self.version)?;
        writer.write_u64::<LittleEndian>(self.offset)?;
        writer.write_u64::<LittleEndian>(self.uncompressed_size)?;
        writer.write_u64::<LittleEndian>(self.compressed_size)?;
        writer.write_u32::<LittleEndian>(self.crc32)?;
        writer.write_u32::<LittleEndian>(self.flags)?;

        writer.write_u8(self.compression as u8)?;
        writer.write_u8(self.comp_level)?;
        writer.write_all(&self.reserved_comp)?;

        writer.write_u8(if self.encrypted { 1 } else { 0 })?;
        writer.write_u8(self.enc_algo)?;
        writer.write_all(&self.key_id)?;
        writer.write_all(&self.chunk_iv)?;
        writer.write_all(&self.auth_tag)?;

        writer.write_u32::<LittleEndian>(self.access_level)?;
        writer.write_u32::<LittleEndian>(self.owner_id)?;

        writer.write_all(&self.reserved)?;
        Ok(())
    }

    /// Validate chunk index entry
    pub fn validate(&self) -> Result<()> {
        // Check reserved bytes
        if !is_all_zeros(&self.reserved) || !is_all_zeros(&self.reserved_comp) {
            return Err(FormatError::InvalidChunk {
                chunk_type: ChunkType::from_fourcc(self.chunk_type).to_string(),
                offset: self.offset,
                reason: "Reserved bytes must be zero".to_string(),
            }
            .into());
        }

        // Check sizes
        if self.uncompressed_size > MAX_CHUNK_SIZE {
            return Err(FormatError::InvalidChunkSize {
                chunk: ChunkType::from_fourcc(self.chunk_type).to_string(),
                size: self.uncompressed_size,
                max_size: MAX_CHUNK_SIZE,
            }
            .into());
        }

        // If compressed, compressed size should be less than uncompressed
        if self.compression != CompressionAlgorithm::None
            && self.compressed_size > 0
            && self.compressed_size >= self.uncompressed_size
        {
            return Err(FormatError::InvalidChunk {
                chunk_type: ChunkType::from_fourcc(self.chunk_type).to_string(),
                offset: self.offset,
                reason: "Compressed size should be less than uncompressed size".to_string(),
            }
            .into());
        }

        // Check encryption algorithm
        if self.encrypted && self.enc_algo > 3 {
            return Err(FormatError::InvalidChunk {
                chunk_type: ChunkType::from_fourcc(self.chunk_type).to_string(),
                offset: self.offset,
                reason: format!("Invalid encryption algorithm: {}", self.enc_algo),
            }
            .into());
        }

        Ok(())
    }

    /// Get the actual size on disk
    pub fn size_on_disk(&self) -> u64 {
        if self.compressed_size > 0 {
            self.compressed_size
        } else {
            self.uncompressed_size
        }
    }

    /// Check if chunk is compressed
    pub fn is_compressed(&self) -> bool {
        self.compression != CompressionAlgorithm::None
            && self.compression != CompressionAlgorithm::Inherit
            && self.compressed_size > 0
    }
}

impl Default for ChunkIndexEntry {
    fn default() -> Self {
        Self::new(*b"    ")
    }
}

/// In-memory chunk representation
#[derive(Debug, Clone)]
pub struct Chunk {
    pub index: ChunkIndexEntry,
    pub data: Vec<u8>,
}

impl Chunk {
    /// Create a new chunk
    pub fn new(chunk_type: ChunkType, data: Vec<u8>) -> Self {
        let mut index = ChunkIndexEntry::new(chunk_type.as_fourcc());
        index.uncompressed_size = data.len() as u64;
        index.crc32 = crc32(&data);

        Chunk { index, data }
    }

    /// Get chunk type
    pub fn chunk_type(&self) -> ChunkType {
        ChunkType::from_fourcc(self.index.chunk_type)
    }

    /// Check if encrypted
    pub fn is_encrypted(&self) -> bool {
        self.index.encrypted
    }

    /// Get required access level
    pub fn access_level(&self) -> u32 {
        self.index.access_level
    }

    /// Get data size
    pub fn size(&self) -> usize {
        self.data.len()
    }

    /// Verify CRC32
    pub fn verify_crc(&self) -> bool {
        crc32(&self.data) == self.index.crc32
    }

    /// Update CRC32
    pub fn update_crc(&mut self) {
        self.index.crc32 = crc32(&self.data);
    }
}

/// Chunk table for managing multiple chunks
#[derive(Debug)]
pub struct ChunkTable {
    entries: Vec<ChunkIndexEntry>,
}

impl ChunkTable {
    /// Create an empty chunk table
    pub fn new() -> Self {
        ChunkTable {
            entries: Vec::new(),
        }
    }

    /// Add a chunk index entry
    pub fn add(&mut self, entry: ChunkIndexEntry) {
        self.entries.push(entry);
    }

    /// Get number of chunks
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Check if empty
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Find chunk by type
    pub fn find_by_type(&self, chunk_type: ChunkType) -> Option<&ChunkIndexEntry> {
        let fourcc = chunk_type.as_fourcc();
        self.entries.iter().find(|e| e.chunk_type == fourcc)
    }

    /// Find all chunks of a given type
    pub fn find_all_by_type(&self, chunk_type: ChunkType) -> Vec<&ChunkIndexEntry> {
        let fourcc = chunk_type.as_fourcc();
        self.entries
            .iter()
            .filter(|e| e.chunk_type == fourcc)
            .collect()
    }

    /// Get chunk by index
    pub fn get(&self, index: usize) -> Option<&ChunkIndexEntry> {
        self.entries.get(index)
    }

    /// Get mutable chunk by index
    pub fn get_mut(&mut self, index: usize) -> Option<&mut ChunkIndexEntry> {
        self.entries.get_mut(index)
    }

    /// Iterate over all entries
    pub fn iter(&self) -> std::slice::Iter<'_, ChunkIndexEntry> {
        self.entries.iter()
    }

    /// Read chunk table from file
    pub fn read_from<R: Read + Seek>(reader: &mut R, offset: u64, count: u32) -> Result<Self> {
        reader.seek(SeekFrom::Start(offset))?;

        let mut table = ChunkTable::new();
        for i in 0..count {
            match ChunkIndexEntry::read_from(reader) {
                Ok(entry) => table.add(entry),
                Err(e) => {
                    return Err(FormatError::InvalidChunk {
                        chunk_type: "index".to_string(),
                        offset: offset + (i as u64 * CHUNK_INDEX_ENTRY_SIZE as u64),
                        reason: format!("Failed to read entry {}: {}", i, e),
                    }
                    .into())
                }
            }
        }

        Ok(table)
    }

    /// Write chunk table to file
    pub fn write_to<W: Write>(&self, writer: &mut W) -> Result<()> {
        for entry in &self.entries {
            entry.write_to(writer)?;
        }
        Ok(())
    }

    /// Validate all entries
    pub fn validate(&self) -> Result<()> {
        // Check for required chunks
        let has_meta = self.find_by_type(ChunkType::META).is_some();
        let has_geom = self.find_by_type(ChunkType::GEOM).is_some();

        if !has_meta {
            return Err(FormatError::MissingRequiredChunk {
                chunk_type: "META".to_string(),
            }
            .into());
        }

        if !has_geom {
            return Err(FormatError::MissingRequiredChunk {
                chunk_type: "GEOM".to_string(),
            }
            .into());
        }

        // Validate each entry
        for (i, entry) in self.entries.iter().enumerate() {
            entry.validate().map_err(|e| RosFileError::Other {
                message: format!("Invalid chunk at index {}: {}", i, e),
                source: None,
            })?;
        }

        // Check for offset overlaps
        let mut sorted_entries = self.entries.clone();
        sorted_entries.sort_by_key(|e| e.offset);

        for i in 1..sorted_entries.len() {
            let prev = &sorted_entries[i - 1];
            let curr = &sorted_entries[i];

            let prev_end = prev.offset + prev.size_on_disk();
            if prev_end > curr.offset {
                return Err(FormatError::InvalidChunk {
                    chunk_type: ChunkType::from_fourcc(curr.chunk_type).to_string(),
                    offset: curr.offset,
                    reason: format!("Overlaps with previous chunk ending at {}", prev_end),
                }
                .into());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_chunk_type_conversions() {
        let chunk_type = ChunkType::GEOM;
        assert_eq!(chunk_type.as_fourcc(), *b"GEOM");
        assert_eq!(chunk_type.as_str(), "GEOM");
        assert!(chunk_type.should_encrypt());
        assert!(chunk_type.is_required());

        let custom = ChunkType::CUSTOM(*b"CUST");
        assert_eq!(custom.as_fourcc(), *b"CUST");
        assert!(!custom.is_required());
    }

    #[test]
    fn test_chunk_index_entry_roundtrip() {
        let mut entry = ChunkIndexEntry::new(*b"TEST");
        entry.version = 2;
        entry.offset = 1024;
        entry.uncompressed_size = 4096;
        entry.compressed_size = 2048;
        entry.crc32 = 0x12345678;
        entry.encrypted = true;
        entry.key_id = [1; 16];
        entry.chunk_iv = [2; 12];

        let mut buffer = Vec::new();
        entry.write_to(&mut buffer).unwrap();
        assert_eq!(buffer.len(), CHUNK_INDEX_ENTRY_SIZE);

        let mut cursor = Cursor::new(buffer);
        let read_entry = ChunkIndexEntry::read_from(&mut cursor).unwrap();

        assert_eq!(read_entry.chunk_type, entry.chunk_type);
        assert_eq!(read_entry.version, entry.version);
        assert_eq!(read_entry.offset, entry.offset);
        assert_eq!(read_entry.encrypted, entry.encrypted);
        assert_eq!(read_entry.key_id, entry.key_id);
    }

    #[test]
    fn test_chunk_validation() {
        let mut entry = ChunkIndexEntry::new(*b"TEST");
        assert!(entry.validate().is_ok());

        // Test invalid size
        entry.uncompressed_size = MAX_CHUNK_SIZE + 1;
        assert!(entry.validate().is_err());

        // Test invalid compression
        entry.uncompressed_size = 1000;
        entry.compressed_size = 2000;
        entry.compression = CompressionAlgorithm::Zstd;
        assert!(entry.validate().is_err());

        // Test invalid reserved bytes
        entry.compressed_size = 500;
        entry.reserved[0] = 1;
        assert!(entry.validate().is_err());
    }

    #[test]
    fn test_chunk_table() {
        let mut table = ChunkTable::new();

        let meta = ChunkIndexEntry::new(ChunkType::META.as_fourcc());
        let geom = ChunkIndexEntry::new(ChunkType::GEOM.as_fourcc());
        let aipr = ChunkIndexEntry::new(ChunkType::AIPR.as_fourcc());

        table.add(meta);
        table.add(geom);
        table.add(aipr);

        assert_eq!(table.len(), 3);
        assert!(table.find_by_type(ChunkType::META).is_some());
        assert!(table.find_by_type(ChunkType::KEYS).is_none());

        let geom_chunks = table.find_all_by_type(ChunkType::GEOM);
        assert_eq!(geom_chunks.len(), 1);
    }

    #[test]
    fn test_chunk_table_validation() {
        let mut table = ChunkTable::new();

        // Missing required chunks
        assert!(table.validate().is_err());

        // Add required chunks
        let mut meta = ChunkIndexEntry::new(ChunkType::META.as_fourcc());
        meta.offset = 1000;
        meta.uncompressed_size = 100;

        let mut geom = ChunkIndexEntry::new(ChunkType::GEOM.as_fourcc());
        geom.offset = 1100;
        geom.uncompressed_size = 200;

        table.add(meta);
        table.add(geom);

        assert!(table.validate().is_ok());

        // Test overlapping chunks
        let mut overlap = ChunkIndexEntry::new(ChunkType::AIPR.as_fourcc());
        overlap.offset = 1150; // Overlaps with GEOM
        overlap.uncompressed_size = 100;
        table.add(overlap);

        assert!(table.validate().is_err());
    }

    #[test]
    fn test_chunk_creation() {
        let data = vec![1, 2, 3, 4, 5];
        let chunk = Chunk::new(ChunkType::AIPR, data.clone());

        assert_eq!(chunk.chunk_type(), ChunkType::AIPR);
        assert_eq!(chunk.size(), 5);
        assert!(chunk.verify_crc());

        // Modify data and check CRC fails
        let mut bad_chunk = chunk.clone();
        bad_chunk.data[0] = 99;
        assert!(!bad_chunk.verify_crc());

        // Update CRC
        bad_chunk.update_crc();
        assert!(bad_chunk.verify_crc());
    }
}
