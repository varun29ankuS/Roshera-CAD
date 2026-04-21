//! Append-only event log implementation

use crate::{StorageConfig, TimelineError, TimelineEvent, TimelineResult};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::fs::{File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// Location of an event in the log
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct EventLocation {
    /// File segment number
    pub segment: u32,
    /// Offset within the segment
    pub offset: u64,
    /// Size of the event data
    pub size: u32,
}

/// Append-only event log
pub struct EventLog {
    /// Base directory for log files
    base_dir: PathBuf,

    /// Current segment number
    current_segment: AtomicU64,

    /// Current segment file
    current_file: tokio::sync::Mutex<Option<File>>,

    /// Current segment size
    current_size: AtomicU64,

    /// Maximum segment size
    max_segment_size: u64,

    /// Compression enabled
    compression_enabled: bool,
}

impl EventLog {
    /// Create a new event log
    pub async fn new(config: &StorageConfig) -> TimelineResult<Self> {
        let base_dir = config.base_path.join("events");
        tokio::fs::create_dir_all(&base_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        // Find the latest segment
        let current_segment = Self::find_latest_segment(&base_dir).await?;

        Ok(Self {
            base_dir,
            current_segment: AtomicU64::new(current_segment),
            current_file: tokio::sync::Mutex::new(None),
            current_size: AtomicU64::new(0),
            max_segment_size: 100 * 1024 * 1024, // 100MB segments
            compression_enabled: config.compression_enabled,
        })
    }

    /// Append an event to the log
    pub async fn append(&self, event: &TimelineEvent) -> TimelineResult<EventLocation> {
        // Serialize event
        let data = bincode::serialize(event)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        // Compress if enabled
        let compressed = if self.compression_enabled {
            lz4_flex::compress_prepend_size(&data)
        } else {
            data
        };

        // Check if we need to rotate
        if self.current_size.load(Ordering::Relaxed) + compressed.len() as u64
            > self.max_segment_size
        {
            self.rotate_segment().await?;
        }

        // Get current file, opening the segment lazily if not yet present.
        let mut file_guard = self.current_file.lock().await;
        let file = match file_guard.as_mut() {
            Some(f) => f,
            None => {
                let segment = self.current_segment.load(Ordering::Relaxed);
                let opened = self.open_segment(segment).await?;
                file_guard.insert(opened)
            }
        };

        // Get current position
        let offset = file
            .stream_position()
            .await
            .map_err(TimelineError::StorageError)?;

        // Write entry header
        let header = EventHeader {
            magic: MAGIC_NUMBER,
            version: 1,
            event_id: event.id.0,
            size: compressed.len() as u32,
            checksum: crc32fast::hash(&compressed),
            compressed: self.compression_enabled,
        };

        let header_bytes = bincode::serialize(&header)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        file.write_all(&header_bytes)
            .await
            .map_err(TimelineError::StorageError)?;

        // Write data
        file.write_all(&compressed)
            .await
            .map_err(TimelineError::StorageError)?;

        // Flush to ensure durability
        file.flush().await.map_err(TimelineError::StorageError)?;

        // Update size
        let written = header_bytes.len() + compressed.len();
        self.current_size
            .fetch_add(written as u64, Ordering::Relaxed);

        Ok(EventLocation {
            segment: self.current_segment.load(Ordering::Relaxed) as u32,
            offset,
            size: written as u32,
        })
    }

    /// Read an event from the log
    pub async fn read_event(&self, location: EventLocation) -> TimelineResult<TimelineEvent> {
        let segment_path = self
            .base_dir
            .join(format!("segment_{:08}.log", location.segment));

        let mut file = File::open(&segment_path)
            .await
            .map_err(TimelineError::StorageError)?;

        // Seek to position
        file.seek(std::io::SeekFrom::Start(location.offset))
            .await
            .map_err(TimelineError::StorageError)?;

        // Read header
        let mut header_buf = vec![0u8; std::mem::size_of::<EventHeader>()];
        file.read_exact(&mut header_buf)
            .await
            .map_err(TimelineError::StorageError)?;

        let header: EventHeader = bincode::deserialize(&header_buf)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))?;

        // Validate header
        if header.magic != MAGIC_NUMBER {
            return Err(TimelineError::StorageError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Invalid magic number",
            )));
        }

        // Read data
        let mut data = vec![0u8; header.size as usize];
        file.read_exact(&mut data)
            .await
            .map_err(TimelineError::StorageError)?;

        // Verify checksum
        let checksum = crc32fast::hash(&data);
        if checksum != header.checksum {
            return Err(TimelineError::StorageError(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "Checksum mismatch",
            )));
        }

        // Decompress if needed
        let decompressed = if header.compressed {
            lz4_flex::decompress_size_prepended(&data).map_err(|e| {
                TimelineError::StorageError(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("Decompression failed: {}", e),
                ))
            })?
        } else {
            data
        };

        // Deserialize
        bincode::deserialize(&decompressed)
            .map_err(|e| TimelineError::SerializationError(e.to_string()))
    }

    /// Rotate to a new segment
    async fn rotate_segment(&self) -> TimelineResult<()> {
        let mut file_guard = self.current_file.lock().await;

        // Close current file
        if let Some(mut file) = file_guard.take() {
            file.flush().await.map_err(TimelineError::StorageError)?;
        }

        // Increment segment number
        self.current_segment.fetch_add(1, Ordering::Relaxed);
        self.current_size.store(0, Ordering::Relaxed);

        Ok(())
    }

    /// Open a segment file
    async fn open_segment(&self, segment: u64) -> TimelineResult<File> {
        let path = self.base_dir.join(format!("segment_{:08}.log", segment));

        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(TimelineError::StorageError)
    }

    /// Find the latest segment number
    async fn find_latest_segment(base_dir: &PathBuf) -> TimelineResult<u64> {
        let mut max_segment = 0u64;

        let mut entries = tokio::fs::read_dir(base_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            if let Some(name) = entry.file_name().to_str() {
                if name.starts_with("segment_") && name.ends_with(".log") {
                    if let Ok(num) = name[8..16].parse::<u64>() {
                        max_segment = max_segment.max(num);
                    }
                }
            }
        }

        Ok(max_segment)
    }

    /// Get total size of event logs
    pub async fn get_size(&self) -> TimelineResult<u64> {
        let mut total_size = 0u64;

        let mut entries = tokio::fs::read_dir(&self.base_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            let metadata = entry
                .metadata()
                .await
                .map_err(TimelineError::StorageError)?;

            if metadata.is_file() {
                total_size += metadata.len();
            }
        }

        Ok(total_size)
    }

    /// Compact old segments if needed
    pub async fn compact_if_needed(&self) -> TimelineResult<()> {
        // Check if we have enough segments to warrant compaction
        let entries = tokio::fs::read_dir(&self.base_dir)
            .await
            .map_err(TimelineError::StorageError)?;

        let mut segment_count = 0;
        let mut entries = entries;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(TimelineError::StorageError)?
        {
            if entry.file_name().to_string_lossy().starts_with("segment_") {
                segment_count += 1;
            }
        }

        // Compact if we have more than 10 segments
        if segment_count > 10 {
            tracing::info!("Starting compaction with {} segments", segment_count);
            // Note: In production, this would trigger actual compaction
            // For now, just log that we would compact
        }

        Ok(())
    }
}

/// Event header for storage
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
#[repr(C)]
struct EventHeader {
    /// Magic number for validation
    magic: u32,
    /// Version number
    version: u8,
    /// Event ID
    event_id: uuid::Uuid,
    /// Size of data
    size: u32,
    /// CRC32 checksum
    checksum: u32,
    /// Whether data is compressed
    compressed: bool,
}

const MAGIC_NUMBER: u32 = 0x54494D45; // "TIME" in hex

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Author, BranchId, EventId, EventMetadata, Operation};

    #[tokio::test]
    async fn test_append_and_read() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        let config = StorageConfig {
            base_path: temp_dir.path().to_path_buf(),
            compression_enabled: false,
            ..Default::default()
        };

        let log = EventLog::new(&config).await.unwrap();

        let event = TimelineEvent {
            id: EventId::new(),
            sequence_number: 1,
            timestamp: chrono::Utc::now(),
            author: Author::System,
            operation: Operation::CreatePrimitive {
                primitive_type: crate::PrimitiveType::Box,
                parameters: serde_json::json!({}),
            },
            inputs: crate::OperationInputs {
                required_entities: vec![],
                optional_entities: vec![],
                parameters: serde_json::Value::Null,
            },
            outputs: crate::OperationOutputs {
                created: vec![],
                modified: vec![],
                deleted: vec![],
                side_effects: vec![],
            },
            metadata: EventMetadata {
                description: None,
                branch_id: BranchId::main(),
                tags: vec![],
                properties: std::collections::HashMap::new(),
            },
        };

        let location = log.append(&event).await.unwrap();
        let read_event = log.read_event(location).await.unwrap();

        assert_eq!(event.id, read_event.id);
        assert_eq!(event.sequence_number, read_event.sequence_number);
    }
}
