//! Memory-mapped storage for efficient large data handling
//!
//! Provides zero-copy access to large files with:
//! - Page-aligned access
//! - Concurrent reads
//! - Write-ahead logging
//! - Crash recovery

use memmap2::{Mmap, MmapMut, MmapOptions};
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

/// Memory-mapped storage engine
pub struct MmapStorage {
    /// Base directory for storage files
    base_path: PathBuf,
    /// Open memory maps
    mmaps: Arc<DashMap<String, Arc<MmapHandle>>>,
    /// Metadata index
    metadata: Arc<RwLock<StorageMetadata>>,
    /// Write-ahead log
    wal: Arc<WriteAheadLog>,
    /// Page size for alignment
    page_size: usize,
}

/// Handle to a memory-mapped file
pub struct MmapHandle {
    /// Memory map
    mmap: Mmap,
    /// File handle
    file: File,
    /// File path
    path: PathBuf,
    /// Size in bytes
    size: usize,
    /// Last accessed time
    last_accessed: std::sync::atomic::AtomicU64,
}

/// Storage metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetadata {
    /// File registry
    files: BTreeMap<String, FileMetadata>,
    /// Total size
    total_size: usize,
    /// Number of files
    file_count: usize,
}

/// File metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    /// File ID
    id: String,
    /// File path
    path: String,
    /// Size in bytes
    size: usize,
    /// Creation time
    created: i64,
    /// Last modified
    modified: i64,
    /// Checksum
    checksum: [u8; 32],
}

/// Write-ahead log for durability
pub struct WriteAheadLog {
    /// Current segment
    current_segment: Arc<RwLock<WalSegment>>,
    /// Segment directory
    segment_dir: PathBuf,
    /// Segment size limit
    segment_size: usize,
}

/// WAL segment
pub struct WalSegment {
    /// Segment ID
    id: u64,
    /// Memory-mapped file
    mmap: MmapMut,
    /// Current position
    position: usize,
    /// File handle
    file: File,
}

/// WAL entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    /// Operation type
    op_type: WalOperation,
    /// Timestamp
    timestamp: i64,
    /// Data
    data: Vec<u8>,
}

/// WAL operation types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WalOperation {
    Write { key: String, offset: u64, length: u64 },
    Delete { key: String },
    Checkpoint,
}

impl MmapStorage {
    /// Create new memory-mapped storage
    pub fn new(base_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_path)?;
        
        let metadata_path = base_path.join("metadata.bin");
        let metadata = if metadata_path.exists() {
            let data = std::fs::read(&metadata_path)?;
            bincode::deserialize(&data)?
        } else {
            StorageMetadata {
                files: BTreeMap::new(),
                total_size: 0,
                file_count: 0,
            }
        };
        
        Ok(Self {
            base_path: base_path.clone(),
            mmaps: Arc::new(DashMap::new()),
            metadata: Arc::new(RwLock::new(metadata)),
            wal: Arc::new(WriteAheadLog::new(base_path.join("wal"))?),
            page_size: 4096, // Standard page size
        })
    }

    /// Create or open a memory-mapped file
    pub async fn create_mmap(&self, key: &str, size: usize) -> anyhow::Result<Arc<MmapHandle>> {
        // Check if already exists
        if let Some(handle) = self.mmaps.get(key) {
            return Ok(handle.clone());
        }
        
        let file_path = self.base_path.join(format!("{}.dat", key));
        
        // Create or open file
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&file_path)?;
        
        // Resize file if needed
        let current_size = file.metadata()?.len() as usize;
        if current_size < size {
            file.set_len(size as u64)?;
        }
        
        // Create memory map
        let mmap = unsafe { MmapOptions::new().map(&file)? };
        
        let handle = Arc::new(MmapHandle {
            mmap,
            file,
            path: file_path.clone(),
            size,
            last_accessed: std::sync::atomic::AtomicU64::new(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            ),
        });
        
        // Update metadata
        let mut metadata = self.metadata.write().await;
        metadata.files.insert(key.to_string(), FileMetadata {
            id: key.to_string(),
            path: file_path.to_string_lossy().to_string(),
            size,
            created: chrono::Utc::now().timestamp(),
            modified: chrono::Utc::now().timestamp(),
            checksum: [0; 32],
        });
        metadata.total_size += size;
        metadata.file_count += 1;
        
        // Store handle
        self.mmaps.insert(key.to_string(), handle.clone());
        
        Ok(handle)
    }

    /// Read data from memory-mapped file
    pub async fn read(&self, key: &str, offset: usize, length: usize) -> anyhow::Result<Vec<u8>> {
        let handle = self.mmaps.get(key)
            .ok_or_else(|| anyhow::anyhow!("Key not found: {}", key))?;
        
        // Update access time
        handle.last_accessed.store(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs(),
            std::sync::atomic::Ordering::Relaxed
        );
        
        // Bounds check
        if offset + length > handle.size {
            return Err(anyhow::anyhow!("Read out of bounds"));
        }
        
        // Read data (zero-copy if possible)
        let data = handle.mmap[offset..offset + length].to_vec();
        Ok(data)
    }

    /// Write data with WAL
    pub async fn write(&self, key: &str, offset: usize, data: &[u8]) -> anyhow::Result<()> {
        // Write to WAL first
        self.wal.append(WalEntry {
            op_type: WalOperation::Write {
                key: key.to_string(),
                offset: offset as u64,
                length: data.len() as u64,
            },
            timestamp: chrono::Utc::now().timestamp(),
            data: data.to_vec(),
        }).await?;
        
        // Get or create mmap
        let handle = if let Some(h) = self.mmaps.get(key) {
            h.clone()
        } else {
            self.create_mmap(key, offset + data.len()).await?
        };
        
        // Write data (requires mutable mmap)
        // In production, use MmapMut for write operations
        unsafe {
            let mut mmap_mut = MmapOptions::new().map_mut(&handle.file)?;
            mmap_mut[offset..offset + data.len()].copy_from_slice(data);
            mmap_mut.flush()?;
        }
        
        Ok(())
    }

    /// Batch read with prefetching
    pub async fn batch_read(&self, requests: Vec<ReadRequest>) -> Vec<anyhow::Result<Vec<u8>>> {
        let mut results = Vec::with_capacity(requests.len());
        
        for request in requests {
            results.push(self.read(&request.key, request.offset, request.length).await);
        }
        
        results
    }

    /// Get storage statistics
    pub async fn stats(&self) -> StorageStats {
        let metadata = self.metadata.read().await;
        let mut active_mmaps = 0;
        let mut total_mapped_size = 0;
        
        for entry in self.mmaps.iter() {
            active_mmaps += 1;
            total_mapped_size += entry.value().size;
        }
        
        StorageStats {
            total_files: metadata.file_count,
            total_size: metadata.total_size,
            active_mmaps,
            total_mapped_size,
            page_size: self.page_size,
        }
    }

    /// Compact storage by removing deleted entries
    pub async fn compact(&self) -> anyhow::Result<()> {
        // Checkpoint WAL
        self.wal.checkpoint().await?;
        
        // Remove deleted files
        let metadata = self.metadata.read().await;
        for (key, file_meta) in &metadata.files {
            let path = Path::new(&file_meta.path);
            if !path.exists() {
                continue;
            }
            
            // Check if file is still referenced
            if !self.mmaps.contains_key(key) {
                // Safe to remove
                std::fs::remove_file(path)?;
            }
        }
        
        Ok(())
    }
}

impl WriteAheadLog {
    /// Create new WAL
    pub fn new(dir: impl AsRef<Path>) -> anyhow::Result<Self> {
        let segment_dir = dir.as_ref().to_path_buf();
        std::fs::create_dir_all(&segment_dir)?;
        
        let segment_path = segment_dir.join("segment_0.wal");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&segment_path)?;
        
        // Pre-allocate 16MB
        let segment_size = 16 * 1024 * 1024;
        file.set_len(segment_size as u64)?;
        
        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };
        
        Ok(Self {
            current_segment: Arc::new(RwLock::new(WalSegment {
                id: 0,
                mmap,
                position: 0,
                file,
            })),
            segment_dir,
            segment_size,
        })
    }

    /// Append entry to WAL
    pub async fn append(&self, entry: WalEntry) -> anyhow::Result<()> {
        let data = bincode::serialize(&entry)?;
        let mut segment = self.current_segment.write().await;
        
        // Check if we need to rotate
        if segment.position + data.len() > self.segment_size {
            self.rotate_segment(&mut segment)?;
        }
        
        // Write entry (fix borrow issue by storing position first)
        let pos = segment.position;
        let end_pos = pos + data.len();
        segment.mmap[pos..end_pos].copy_from_slice(&data);
        segment.position = end_pos;
        
        // Flush to disk
        segment.mmap.flush_async()?;
        
        Ok(())
    }

    /// Rotate to new segment
    fn rotate_segment(&self, segment: &mut WalSegment) -> anyhow::Result<()> {
        segment.id += 1;
        let segment_path = self.segment_dir.join(format!("segment_{}.wal", segment.id));
        
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&segment_path)?;
        
        file.set_len(self.segment_size as u64)?;
        let mmap = unsafe { MmapOptions::new().map_mut(&file)? };
        
        segment.mmap = mmap;
        segment.file = file;
        segment.position = 0;
        
        Ok(())
    }

    /// Checkpoint WAL
    pub async fn checkpoint(&self) -> anyhow::Result<()> {
        let segment = self.current_segment.read().await;
        segment.mmap.flush()?;
        Ok(())
    }

    /// Replay WAL for recovery
    pub async fn replay(&self) -> anyhow::Result<Vec<WalEntry>> {
        let mut entries = Vec::new();
        
        for entry in std::fs::read_dir(&self.segment_dir)? {
            let entry = entry?;
            if entry.path().extension() == Some(std::ffi::OsStr::new("wal")) {
                let data = std::fs::read(entry.path())?;
                let mut pos = 0;
                
                while pos < data.len() {
                    if let Ok(entry) = bincode::deserialize::<WalEntry>(&data[pos..]) {
                        let entry_size = bincode::serialized_size(&entry)? as usize;
                        entries.push(entry);
                        pos += entry_size;
                    } else {
                        break;
                    }
                }
            }
        }
        
        Ok(entries)
    }
}

/// Read request for batch operations
#[derive(Debug, Clone)]
pub struct ReadRequest {
    pub key: String,
    pub offset: usize,
    pub length: usize,
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub total_files: usize,
    pub total_size: usize,
    pub active_mmaps: usize,
    pub total_mapped_size: usize,
    pub page_size: usize,
}

// Safe wrapper for concurrent access
unsafe impl Send for MmapHandle {}
unsafe impl Sync for MmapHandle {}
unsafe impl Send for WalSegment {}
unsafe impl Sync for WalSegment {}

use dashmap::DashMap;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mmap_storage() {
        let dir = tempfile::tempdir().unwrap();
        let storage = MmapStorage::new(dir.path()).unwrap();
        
        // Write data
        let data = b"Hello, Memory Mapped World!";
        storage.write("test_key", 0, data).await.unwrap();
        
        // Read data
        let read_data = storage.read("test_key", 0, data.len()).await.unwrap();
        assert_eq!(read_data, data);
    }

    #[tokio::test]
    async fn test_wal_recovery() {
        let dir = tempfile::tempdir().unwrap();
        let wal = WriteAheadLog::new(dir.path()).unwrap();
        
        // Write entries
        for i in 0..10 {
            wal.append(WalEntry {
                op_type: WalOperation::Write {
                    key: format!("key_{}", i),
                    offset: 0,
                    length: 100,
                },
                timestamp: i,
                data: vec![i as u8; 100],
            }).await.unwrap();
        }
        
        // Checkpoint
        wal.checkpoint().await.unwrap();
        
        // Replay
        let entries = wal.replay().await.unwrap();
        assert!(entries.len() >= 10);
    }
}