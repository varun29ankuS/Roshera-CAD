//! Split management for immutable data chunks
//!
//! Implements Quickwit-style immutable splits for:
//! - Efficient versioning
//! - Fast range queries
//! - Atomic updates
//! - Compaction

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Split manager
pub struct SplitManager {
    splits: Arc<RwLock<BTreeMap<SplitId, Split>>>,
    active_split: Arc<RwLock<Option<SplitId>>>,
    split_metadata: Arc<RwLock<SplitMetadata>>,
    compactor: Arc<Compactor>,
}

/// Individual split
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Split {
    pub id: SplitId,
    pub state: SplitState,
    pub metadata: SplitInfo,
    pub data_path: PathBuf,
    pub index_path: PathBuf,
}

/// Split ID
pub type SplitId = uuid::Uuid;

/// Split state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SplitState {
    Active,
    Sealed,
    Merging,
    Deleted,
}

/// Split information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitInfo {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sealed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub doc_count: usize,
    pub size_bytes: usize,
    pub time_range: Option<TimeRange>,
    pub checksum: [u8; 32],
}

/// Time range
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: chrono::DateTime<chrono::Utc>,
    pub end: chrono::DateTime<chrono::Utc>,
}

/// Split metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitMetadata {
    pub total_splits: usize,
    pub active_splits: usize,
    pub sealed_splits: usize,
    pub total_docs: usize,
    pub total_size_bytes: usize,
}

/// Compactor for merging splits
pub struct Compactor {
    strategy: CompactionStrategy,
    min_splits: usize,
    max_split_size: usize,
}

/// Compaction strategy
#[derive(Debug, Clone)]
pub enum CompactionStrategy {
    SizeTiered,
    TimeTiered,
    Leveled,
}

impl SplitManager {
    /// Create new split manager
    pub fn new() -> Self {
        Self {
            splits: Arc::new(RwLock::new(BTreeMap::new())),
            active_split: Arc::new(RwLock::new(None)),
            split_metadata: Arc::new(RwLock::new(SplitMetadata {
                total_splits: 0,
                active_splits: 0,
                sealed_splits: 0,
                total_docs: 0,
                total_size_bytes: 0,
            })),
            compactor: Arc::new(Compactor::new()),
        }
    }

    /// Create new split
    pub async fn create_split(&self) -> anyhow::Result<SplitId> {
        let split_id = uuid::Uuid::new_v4();
        let split = Split {
            id: split_id,
            state: SplitState::Active,
            metadata: SplitInfo {
                created_at: chrono::Utc::now(),
                sealed_at: None,
                doc_count: 0,
                size_bytes: 0,
                time_range: None,
                checksum: [0; 32],
            },
            data_path: PathBuf::from(format!("./rag_data/splits/{}/data", split_id)),
            index_path: PathBuf::from(format!("./rag_data/splits/{}/index", split_id)),
        };

        // Create directories
        tokio::fs::create_dir_all(&split.data_path).await?;
        tokio::fs::create_dir_all(&split.index_path).await?;

        // Add to splits
        self.splits.write().await.insert(split_id, split);
        
        // Update active split
        *self.active_split.write().await = Some(split_id);
        
        // Update metadata
        let mut metadata = self.split_metadata.write().await;
        metadata.total_splits += 1;
        metadata.active_splits += 1;

        Ok(split_id)
    }

    /// Seal a split
    pub async fn seal_split(&self, split_id: SplitId) -> anyhow::Result<()> {
        let mut splits = self.splits.write().await;
        
        if let Some(split) = splits.get_mut(&split_id) {
            split.state = SplitState::Sealed;
            split.metadata.sealed_at = Some(chrono::Utc::now());
            
            // Update metadata
            let mut metadata = self.split_metadata.write().await;
            metadata.active_splits -= 1;
            metadata.sealed_splits += 1;
            
            // Clear active split if this was it
            let mut active = self.active_split.write().await;
            if *active == Some(split_id) {
                *active = None;
            }
        }
        
        Ok(())
    }

    /// Get active split
    pub async fn get_active_split(&self) -> anyhow::Result<Option<Split>> {
        let active_id = self.active_split.read().await;
        
        if let Some(id) = *active_id {
            let splits = self.splits.read().await;
            Ok(splits.get(&id).cloned())
        } else {
            Ok(None)
        }
    }

    /// List splits
    pub async fn list_splits(&self, state: Option<SplitState>) -> Vec<Split> {
        let splits = self.splits.read().await;
        
        if let Some(state) = state {
            splits
                .values()
                .filter(|s| std::mem::discriminant(&s.state) == std::mem::discriminant(&state))
                .cloned()
                .collect()
        } else {
            splits.values().cloned().collect()
        }
    }

    /// Compact splits
    pub async fn compact(&self) -> anyhow::Result<()> {
        let splits = self.list_splits(Some(SplitState::Sealed)).await;
        
        if splits.len() >= self.compactor.min_splits {
            self.compactor.compact(splits).await?;
        }
        
        Ok(())
    }

    /// Get metadata
    pub async fn get_metadata(&self) -> SplitMetadata {
        self.split_metadata.read().await.clone()
    }
}

impl Compactor {
    pub fn new() -> Self {
        Self {
            strategy: CompactionStrategy::SizeTiered,
            min_splits: 5,
            max_split_size: 100 * 1024 * 1024, // 100MB
        }
    }

    pub async fn compact(&self, splits: Vec<Split>) -> anyhow::Result<Split> {
        match self.strategy {
            CompactionStrategy::SizeTiered => self.size_tiered_compaction(splits).await,
            CompactionStrategy::TimeTiered => self.time_tiered_compaction(splits).await,
            CompactionStrategy::Leveled => self.leveled_compaction(splits).await,
        }
    }

    async fn size_tiered_compaction(&self, splits: Vec<Split>) -> anyhow::Result<Split> {
        // Sort by size
        let mut sorted = splits;
        sorted.sort_by_key(|s| s.metadata.size_bytes);
        
        // Merge smallest splits
        let to_merge = sorted.iter().take(self.min_splits).cloned().collect();
        self.merge_splits(to_merge).await
    }

    async fn time_tiered_compaction(&self, splits: Vec<Split>) -> anyhow::Result<Split> {
        // Sort by creation time
        let mut sorted = splits;
        sorted.sort_by_key(|s| s.metadata.created_at);
        
        // Merge oldest splits
        let to_merge = sorted.iter().take(self.min_splits).cloned().collect();
        self.merge_splits(to_merge).await
    }

    async fn leveled_compaction(&self, splits: Vec<Split>) -> anyhow::Result<Split> {
        // Implement leveled compaction (like LevelDB)
        // For now, fallback to size-tiered
        self.size_tiered_compaction(splits).await
    }

    async fn merge_splits(&self, splits: Vec<Split>) -> anyhow::Result<Split> {
        let merged_id = uuid::Uuid::new_v4();
        let mut total_docs = 0;
        let mut total_size = 0;
        
        for split in &splits {
            total_docs += split.metadata.doc_count;
            total_size += split.metadata.size_bytes;
        }
        
        let merged = Split {
            id: merged_id,
            state: SplitState::Sealed,
            metadata: SplitInfo {
                created_at: chrono::Utc::now(),
                sealed_at: Some(chrono::Utc::now()),
                doc_count: total_docs,
                size_bytes: total_size,
                time_range: self.calculate_time_range(&splits),
                checksum: [0; 32], // Would calculate actual checksum
            },
            data_path: PathBuf::from(format!("./rag_data/splits/{}/data", merged_id)),
            index_path: PathBuf::from(format!("./rag_data/splits/{}/index", merged_id)),
        };
        
        // Create directories
        tokio::fs::create_dir_all(&merged.data_path).await?;
        tokio::fs::create_dir_all(&merged.index_path).await?;
        
        // Merge data files
        self.merge_data_files(&splits, &merged).await?;
        
        // Merge index files
        self.merge_index_files(&splits, &merged).await?;
        
        Ok(merged)
    }

    fn calculate_time_range(&self, splits: &[Split]) -> Option<TimeRange> {
        let mut min_time = None;
        let mut max_time = None;
        
        for split in splits {
            if let Some(range) = &split.metadata.time_range {
                min_time = Some(min_time.map_or(range.start, |t: chrono::DateTime<chrono::Utc>| t.min(range.start)));
                max_time = Some(max_time.map_or(range.end, |t: chrono::DateTime<chrono::Utc>| t.max(range.end)));
            }
        }
        
        if let (Some(start), Some(end)) = (min_time, max_time) {
            Some(TimeRange { start, end })
        } else {
            None
        }
    }

    async fn merge_data_files(&self, sources: &[Split], target: &Split) -> anyhow::Result<()> {
        // Merge data files from source splits into target
        // This would actually copy and merge the files
        Ok(())
    }

    async fn merge_index_files(&self, sources: &[Split], target: &Split) -> anyhow::Result<()> {
        // Merge index files from source splits into target
        // This would actually merge the indexes
        Ok(())
    }
}

/// Split reader for querying
pub struct SplitReader {
    split: Split,
    data_cache: Option<Vec<u8>>,
}

impl SplitReader {
    /// Create new reader
    pub fn new(split: Split) -> Self {
        Self {
            split,
            data_cache: None,
        }
    }

    /// Read data from split
    pub async fn read_data(&mut self) -> anyhow::Result<Vec<u8>> {
        if let Some(cache) = &self.data_cache {
            return Ok(cache.clone());
        }
        
        let data = tokio::fs::read(&self.split.data_path.join("data.bin")).await?;
        self.data_cache = Some(data.clone());
        Ok(data)
    }

    /// Search in split
    pub async fn search(&self, query: &str) -> anyhow::Result<Vec<SearchResult>> {
        // Search within this split's index
        Ok(vec![])
    }
}

/// Split writer for adding data
pub struct SplitWriter {
    split: Split,
    buffer: Vec<u8>,
    index_buffer: Vec<IndexEntry>,
}

/// Index entry
#[derive(Debug, Clone)]
struct IndexEntry {
    term: String,
    doc_id: u32,
    position: u32,
}

impl SplitWriter {
    /// Create new writer
    pub fn new(split: Split) -> Self {
        Self {
            split,
            buffer: Vec::new(),
            index_buffer: Vec::new(),
        }
    }

    /// Write document
    pub async fn write_document(&mut self, doc: &[u8]) -> anyhow::Result<()> {
        self.buffer.extend_from_slice(doc);
        
        // Update split metadata
        self.split.metadata.doc_count += 1;
        self.split.metadata.size_bytes += doc.len();
        
        // Flush if buffer is large
        if self.buffer.len() > 1024 * 1024 {
            self.flush().await?;
        }
        
        Ok(())
    }

    /// Flush buffer to disk
    pub async fn flush(&mut self) -> anyhow::Result<()> {
        if !self.buffer.is_empty() {
            let data_file = self.split.data_path.join("data.bin");
            tokio::fs::write(&data_file, &self.buffer).await?;
            self.buffer.clear();
        }
        
        if !self.index_buffer.is_empty() {
            // Write index entries
            self.index_buffer.clear();
        }
        
        Ok(())
    }

    /// Seal the split
    pub async fn seal(mut self) -> anyhow::Result<Split> {
        self.flush().await?;
        self.split.state = SplitState::Sealed;
        self.split.metadata.sealed_at = Some(chrono::Utc::now());
        Ok(self.split)
    }
}

/// Search result from split
#[derive(Debug, Clone)]
struct SearchResult {
    doc_id: u32,
    score: f32,
}