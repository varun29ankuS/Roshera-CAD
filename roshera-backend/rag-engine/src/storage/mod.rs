//! TurboStorage - Zero-dependency storage engine with immutable splits
//!
//! Implements a Quickwit-style storage system with:
//! - Immutable data splits for versioning
//! - No external dependencies (no PostgreSQL, no S3)
//! - Built-in compression and deduplication
//! - Atomic operations with MVCC

pub mod compression;
pub mod safe_storage;
// pub mod mmap; // Disabled: requires unsafe code

use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Main storage engine
#[derive(Clone)]
pub struct StorageEngine {
    splits: Arc<SplitManager>,
    metadata: Arc<MetadataStore>,
    compaction: Arc<CompactionEngine>,
    base_path: PathBuf,
}

/// Split manager for immutable data chunks
pub struct SplitManager {
    active_split: Arc<RwLock<Split>>,
    immutable_splits: DashMap<SplitId, Arc<Split>>,
    split_index: Arc<SplitIndex>,
}

/// Individual data split (immutable once sealed)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Split {
    pub id: SplitId,
    pub version: u64,
    pub documents: Vec<Document>,
    pub metadata: SplitMetadata,
    pub sealed: bool,
}

/// Document stored in a split
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Document {
    pub id: DocumentId,
    pub content: Vec<u8>,
    pub metadata: DocumentMetadata,
    pub version: u64,
    pub deleted: bool,
}

/// Split metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitMetadata {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub sealed_at: Option<chrono::DateTime<chrono::Utc>>,
    pub doc_count: usize,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
}

/// Document metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentMetadata {
    pub source: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub tags: Vec<String>,
    pub checksum: [u8; 32],
}

/// Split index for fast lookups
pub struct SplitIndex {
    doc_to_split: DashMap<DocumentId, Vec<(SplitId, u64)>>, // Doc ID -> [(Split ID, Version)]
    time_index: Arc<RwLock<BTreeMap<i64, Vec<SplitId>>>>,   // Timestamp -> Split IDs
}

/// Metadata store for schema and configuration
pub struct MetadataStore {
    schemas: DashMap<String, Schema>,
    configs: DashMap<String, serde_json::Value>,
}

/// Compaction engine for merging splits
pub struct CompactionEngine {
    min_splits: usize,
    max_split_size: usize,
    compaction_interval: std::time::Duration,
}

/// Zero-dependency storage implementation
pub struct TurboStorage {
    data_dir: PathBuf,
    wal: WriteAheadLog,
    block_cache: BlockCache,
}

/// Write-ahead log for durability
struct WriteAheadLog {
    current_segment: Arc<RwLock<WalSegment>>,
    segments: Vec<WalSegment>,
}

/// WAL segment
struct WalSegment {
    id: u64,
    path: PathBuf,
    size: usize,
    entries: Vec<WalEntry>,
}

/// WAL entry
#[derive(Debug, Serialize, Deserialize)]
struct WalEntry {
    op_type: OperationType,
    timestamp: i64,
    data: Vec<u8>,
}

/// Operation type
#[derive(Debug, Serialize, Deserialize)]
enum OperationType {
    Insert,
    Update,
    Delete,
    Commit,
}

/// Block cache for hot data
struct BlockCache {
    blocks: DashMap<BlockId, CachedBlock>,
    size_limit: usize,
    current_size: Arc<RwLock<usize>>,
}

/// Cached block
struct CachedBlock {
    data: Vec<u8>,
    access_count: u64,
    last_access: std::time::Instant,
}

/// Schema definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    pub name: String,
    pub fields: Vec<Field>,
    pub version: u32,
}

/// Field definition
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Field {
    pub name: String,
    pub field_type: FieldType,
    pub indexed: bool,
    pub stored: bool,
}

/// Field type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FieldType {
    Text,
    Integer,
    Float,
    Boolean,
    DateTime,
    Bytes,
}

// Type aliases
type SplitId = uuid::Uuid;
type DocumentId = uuid::Uuid;
type BlockId = u64;

impl StorageEngine {
    /// Create new storage engine
    pub async fn new(base_path: &Path) -> anyhow::Result<Self> {
        tokio::fs::create_dir_all(base_path).await?;
        
        Ok(Self {
            splits: Arc::new(SplitManager::new()),
            metadata: Arc::new(MetadataStore::new()),
            compaction: Arc::new(CompactionEngine::new()),
            base_path: base_path.to_path_buf(),
        })
    }

    /// Write a document (creates new version)
    pub async fn write(&self, doc: Document) -> anyhow::Result<()> {
        self.splits.write(doc).await
    }

    /// Read a document (latest version)
    pub async fn read(&self, id: DocumentId) -> anyhow::Result<Option<Document>> {
        self.splits.read(id).await
    }

    /// Query documents
    pub async fn query(&self, query: Query) -> anyhow::Result<Vec<Document>> {
        self.splits.query(query).await
    }

    /// Compact splits
    pub async fn compact(&self) -> anyhow::Result<()> {
        self.compaction.run(&self.splits).await
    }
}

impl SplitManager {
    pub fn new() -> Self {
        Self {
            active_split: Arc::new(RwLock::new(Split::new())),
            immutable_splits: DashMap::new(),
            split_index: Arc::new(SplitIndex::new()),
        }
    }

    pub async fn write(&self, doc: Document) -> anyhow::Result<()> {
        let mut active = self.active_split.write().await;
        
        // Check if split is full
        if active.documents.len() >= 10000 {
            // Seal current split
            active.sealed = true;
            active.metadata.sealed_at = Some(chrono::Utc::now());
            
            // Move to immutable
            let sealed = active.clone();
            self.immutable_splits.insert(sealed.id, Arc::new(sealed));
            
            // Create new active split
            *active = Split::new();
        }
        
        // Add document to active split
        active.documents.push(doc.clone());
        active.metadata.doc_count += 1;
        
        // Update index
        self.split_index.index_document(&doc, active.id).await;
        
        Ok(())
    }

    pub async fn read(&self, id: DocumentId) -> anyhow::Result<Option<Document>> {
        // Check index for splits containing this document
        if let Some(splits) = self.split_index.doc_to_split.get(&id) {
            // Get latest version
            if let Some((split_id, _version)) = splits.last() {
                // Check active split
                let active = self.active_split.read().await;
                if active.id == *split_id {
                    return Ok(active.documents.iter()
                        .find(|d| d.id == id && !d.deleted)
                        .cloned());
                }
                
                // Check immutable splits
                if let Some(split) = self.immutable_splits.get(split_id) {
                    return Ok(split.documents.iter()
                        .find(|d| d.id == id && !d.deleted)
                        .cloned());
                }
            }
        }
        
        Ok(None)
    }

    pub async fn query(&self, query: Query) -> anyhow::Result<Vec<Document>> {
        let mut results = Vec::new();
        
        // Search active split
        let active = self.active_split.read().await;
        for doc in &active.documents {
            if query.matches(doc) && !doc.deleted {
                results.push(doc.clone());
            }
        }
        
        // Search immutable splits
        for entry in self.immutable_splits.iter() {
            for doc in &entry.value().documents {
                if query.matches(doc) && !doc.deleted {
                    results.push(doc.clone());
                }
            }
        }
        
        Ok(results)
    }
}

impl Split {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            version: 1,
            documents: Vec::new(),
            metadata: SplitMetadata {
                created_at: chrono::Utc::now(),
                sealed_at: None,
                doc_count: 0,
                compressed_size: 0,
                uncompressed_size: 0,
            },
            sealed: false,
        }
    }
}

impl SplitIndex {
    pub fn new() -> Self {
        Self {
            doc_to_split: DashMap::new(),
            time_index: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    pub async fn index_document(&self, doc: &Document, split_id: SplitId) {
        // Update document to split mapping
        self.doc_to_split
            .entry(doc.id)
            .or_insert_with(Vec::new)
            .push((split_id, doc.version));
        
        // Update time index
        let timestamp = doc.metadata.timestamp.timestamp();
        let mut time_index = self.time_index.write().await;
        time_index
            .entry(timestamp)
            .or_insert_with(Vec::new)
            .push(split_id);
    }
}

impl MetadataStore {
    pub fn new() -> Self {
        Self {
            schemas: DashMap::new(),
            configs: DashMap::new(),
        }
    }
}

impl CompactionEngine {
    pub fn new() -> Self {
        Self {
            min_splits: 5,
            max_split_size: 100_000,
            compaction_interval: std::time::Duration::from_secs(3600),
        }
    }

    pub async fn run(&self, splits: &SplitManager) -> anyhow::Result<()> {
        // Find small splits to merge
        let mut small_splits = Vec::new();
        for entry in splits.immutable_splits.iter() {
            if entry.value().metadata.doc_count < 1000 {
                small_splits.push(entry.key().clone());
            }
        }
        
        // Merge if we have enough small splits
        if small_splits.len() >= self.min_splits {
            self.merge_splits(splits, &small_splits).await?;
        }
        
        Ok(())
    }

    async fn merge_splits(&self, splits: &SplitManager, split_ids: &[SplitId]) -> anyhow::Result<()> {
        let mut merged = Split::new();
        
        // Collect all documents
        for split_id in split_ids {
            if let Some(split) = splits.immutable_splits.get(split_id) {
                for doc in &split.documents {
                    if !doc.deleted {
                        merged.documents.push(doc.clone());
                    }
                }
            }
        }
        
        // Seal merged split
        merged.sealed = true;
        merged.metadata.sealed_at = Some(chrono::Utc::now());
        merged.metadata.doc_count = merged.documents.len();
        
        // Add merged split
        splits.immutable_splits.insert(merged.id, Arc::new(merged));
        
        // Remove old splits
        for split_id in split_ids {
            splits.immutable_splits.remove(split_id);
        }
        
        Ok(())
    }
}

/// Query for filtering documents
#[derive(Debug, Clone)]
pub struct Query {
    pub filters: Vec<Filter>,
    pub limit: Option<usize>,
}

/// Filter condition
#[derive(Debug, Clone)]
pub enum Filter {
    Tag(String),
    TimeRange(i64, i64),
    Source(String),
}

impl Query {
    pub fn matches(&self, doc: &Document) -> bool {
        for filter in &self.filters {
            match filter {
                Filter::Tag(tag) => {
                    if !doc.metadata.tags.contains(tag) {
                        return false;
                    }
                }
                Filter::TimeRange(start, end) => {
                    let timestamp = doc.metadata.timestamp.timestamp();
                    if timestamp < *start || timestamp > *end {
                        return false;
                    }
                }
                Filter::Source(source) => {
                    if doc.metadata.source != *source {
                        return false;
                    }
                }
            }
        }
        true
    }
}