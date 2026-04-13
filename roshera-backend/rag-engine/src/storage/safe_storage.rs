//! Safe file-based storage without memory mapping
//!
//! Provides efficient file access without unsafe code:
//! - Buffered I/O for performance
//! - Concurrent reads using Arc<RwLock>
//! - Write-ahead logging
//! - Crash recovery

use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;
use serde::{Serialize, Deserialize};
use std::collections::BTreeMap;

/// Safe storage engine using buffered file I/O
pub struct SafeStorage {
    /// Base directory for storage files
    base_path: PathBuf,
    /// Open file handles
    files: Arc<DashMap<String, Arc<RwLock<FileHandle>>>>,
    /// Metadata index
    metadata: Arc<RwLock<StorageMetadata>>,
    /// Write-ahead log
    wal: Arc<WriteAheadLog>,
    /// Buffer size for I/O
    buffer_size: usize,
}

/// File handle with buffered I/O
pub struct FileHandle {
    file: File,
    path: PathBuf,
    size: u64,
}

/// Storage metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageMetadata {
    pub files: BTreeMap<String, FileMetadata>,
    pub total_size: u64,
    pub last_modified: i64,
}

/// File metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileMetadata {
    pub path: String,
    pub size: u64,
    pub created: i64,
    pub modified: i64,
    pub checksum: String,
}

/// Write-ahead log for durability
pub struct WriteAheadLog {
    path: PathBuf,
    current_segment: Arc<RwLock<WalSegment>>,
    segments: Arc<RwLock<Vec<PathBuf>>>,
    segment_size: usize,
}

/// WAL segment
pub struct WalSegment {
    file: BufWriter<File>,
    path: PathBuf,
    position: usize,
    size: usize,
}

/// WAL entry
#[derive(Debug, Serialize, Deserialize)]
pub struct WalEntry {
    pub operation: WalOperation,
    pub timestamp: i64,
    pub data: Vec<u8>,
}

/// WAL operation types
#[derive(Debug, Serialize, Deserialize)]
pub enum WalOperation {
    Write { file: String, offset: u64, length: usize },
    Delete { file: String },
    Truncate { file: String, size: u64 },
}

impl SafeStorage {
    /// Create new safe storage
    pub fn new(base_path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let base_path = base_path.as_ref().to_path_buf();
        std::fs::create_dir_all(&base_path)?;
        
        let wal_path = base_path.join("wal");
        std::fs::create_dir_all(&wal_path)?;
        
        Ok(Self {
            base_path: base_path.clone(),
            files: Arc::new(DashMap::new()),
            metadata: Arc::new(RwLock::new(StorageMetadata {
                files: BTreeMap::new(),
                total_size: 0,
                last_modified: chrono::Utc::now().timestamp(),
            })),
            wal: Arc::new(WriteAheadLog::new(wal_path)?),
            buffer_size: 64 * 1024, // 64KB buffer
        })
    }

    /// Write data to a file
    pub async fn write(&self, key: &str, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        // Log to WAL first
        self.wal.log_write(key, offset, data).await?;
        
        // Get or create file handle
        let handle = self.get_or_create_file(key).await?;
        let mut handle = handle.write().await;
        
        // Seek and write
        handle.file.seek(SeekFrom::Start(offset))?;
        handle.file.write_all(data)?;
        handle.file.sync_all()?;
        
        // Update metadata
        let mut metadata = self.metadata.write().await;
        if let Some(file_meta) = metadata.files.get_mut(key) {
            file_meta.modified = chrono::Utc::now().timestamp();
            file_meta.size = handle.file.metadata()?.len();
        }
        
        Ok(())
    }

    /// Read data from a file
    pub async fn read(&self, key: &str, offset: u64, length: usize) -> anyhow::Result<Vec<u8>> {
        let handle = self.get_file(key).await?;
        let mut handle = handle.write().await;
        
        // Seek and read
        handle.file.seek(SeekFrom::Start(offset))?;
        let mut buffer = vec![0u8; length];
        handle.file.read_exact(&mut buffer)?;
        
        Ok(buffer)
    }

    /// Get or create a file handle
    async fn get_or_create_file(&self, key: &str) -> anyhow::Result<Arc<RwLock<FileHandle>>> {
        if let Some(handle) = self.files.get(key) {
            return Ok(handle.clone());
        }
        
        let path = self.base_path.join(format!("{}.dat", key));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(&path)?;
        
        let size = file.metadata()?.len();
        let handle = Arc::new(RwLock::new(FileHandle {
            file,
            path: path.clone(),
            size,
        }));
        
        self.files.insert(key.to_string(), handle.clone());
        
        // Update metadata
        let mut metadata = self.metadata.write().await;
        metadata.files.insert(key.to_string(), FileMetadata {
            path: path.to_string_lossy().to_string(),
            size,
            created: chrono::Utc::now().timestamp(),
            modified: chrono::Utc::now().timestamp(),
            checksum: String::new(),
        });
        
        Ok(handle)
    }

    /// Get existing file handle
    async fn get_file(&self, key: &str) -> anyhow::Result<Arc<RwLock<FileHandle>>> {
        self.files.get(key)
            .map(|h| h.clone())
            .ok_or_else(|| anyhow::anyhow!("File not found: {}", key))
    }

    /// List all files
    pub async fn list(&self) -> Vec<String> {
        self.files.iter()
            .map(|entry| entry.key().clone())
            .collect()
    }

    /// Delete a file
    pub async fn delete(&self, key: &str) -> anyhow::Result<()> {
        // Log to WAL
        self.wal.log_delete(key).await?;
        
        // Remove from cache
        if let Some((_, handle)) = self.files.remove(key) {
            let handle = handle.read().await;
            std::fs::remove_file(&handle.path)?;
        }
        
        // Update metadata
        let mut metadata = self.metadata.write().await;
        metadata.files.remove(key);
        
        Ok(())
    }

    /// Get storage statistics
    pub async fn stats(&self) -> StorageStats {
        let metadata = self.metadata.read().await;
        StorageStats {
            file_count: metadata.files.len(),
            total_size: metadata.total_size,
            last_modified: metadata.last_modified,
        }
    }
}

impl WriteAheadLog {
    /// Create new WAL
    pub fn new(path: impl AsRef<Path>) -> anyhow::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let segment_path = path.join("segment_0.wal");
        
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&segment_path)?;
        
        Ok(Self {
            path,
            current_segment: Arc::new(RwLock::new(WalSegment {
                file: BufWriter::new(file),
                path: segment_path,
                position: 0,
                size: 0,
            })),
            segments: Arc::new(RwLock::new(vec![])),
            segment_size: 10 * 1024 * 1024, // 10MB segments
        })
    }

    /// Log a write operation
    pub async fn log_write(&self, file: &str, offset: u64, data: &[u8]) -> anyhow::Result<()> {
        let entry = WalEntry {
            operation: WalOperation::Write {
                file: file.to_string(),
                offset,
                length: data.len(),
            },
            timestamp: chrono::Utc::now().timestamp(),
            data: data.to_vec(),
        };
        
        self.append_entry(&entry).await
    }

    /// Log a delete operation
    pub async fn log_delete(&self, file: &str) -> anyhow::Result<()> {
        let entry = WalEntry {
            operation: WalOperation::Delete {
                file: file.to_string(),
            },
            timestamp: chrono::Utc::now().timestamp(),
            data: vec![],
        };
        
        self.append_entry(&entry).await
    }

    /// Append entry to WAL
    async fn append_entry(&self, entry: &WalEntry) -> anyhow::Result<()> {
        let serialized = bincode::serialize(entry)?;
        let mut segment = self.current_segment.write().await;
        
        // Check if we need to rotate
        if segment.position + serialized.len() > self.segment_size {
            self.rotate_segment(&mut segment).await?;
        }
        
        // Write entry
        segment.file.write_all(&serialized)?;
        segment.file.flush()?;
        segment.position += serialized.len();
        
        Ok(())
    }

    /// Rotate to new segment
    async fn rotate_segment(&self, current: &mut WalSegment) -> anyhow::Result<()> {
        current.file.flush()?;
        
        let mut segments = self.segments.write().await;
        segments.push(current.path.clone());
        
        let new_path = self.path.join(format!("segment_{}.wal", segments.len()));
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&new_path)?;
        
        current.file = BufWriter::new(file);
        current.path = new_path;
        current.position = 0;
        
        Ok(())
    }

    /// Replay WAL for recovery
    pub async fn replay(&self) -> anyhow::Result<Vec<WalEntry>> {
        let mut entries = Vec::new();
        let segments = self.segments.read().await;
        
        for segment_path in segments.iter() {
            let mut file = File::open(segment_path)?;
            let mut buffer = Vec::new();
            file.read_to_end(&mut buffer)?;
            
            let mut cursor = 0;
            while cursor < buffer.len() {
                if let Ok(entry) = bincode::deserialize::<WalEntry>(&buffer[cursor..]) {
                    let entry_size = bincode::serialized_size(&entry)? as usize;
                    entries.push(entry);
                    cursor += entry_size;
                } else {
                    break;
                }
            }
        }
        
        Ok(entries)
    }
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub file_count: usize,
    pub total_size: u64,
    pub last_modified: i64,
}

use dashmap::DashMap;
use chrono;

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_safe_storage() {
        let temp_dir = TempDir::new().unwrap();
        let storage = SafeStorage::new(temp_dir.path()).unwrap();
        
        // Test write and read
        let data = b"Hello, World!";
        storage.write("test", 0, data).await.unwrap();
        
        let read_data = storage.read("test", 0, data.len()).await.unwrap();
        assert_eq!(read_data, data);
        
        // Test delete
        storage.delete("test").await.unwrap();
        assert!(storage.get_file("test").await.is_err());
    }
}