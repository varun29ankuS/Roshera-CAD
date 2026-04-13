/// Enterprise File Indexing System for TurboRAG
/// 
/// Indexes codebases with intelligent chunking, language detection,
/// and real-time file watching

use std::path::{Path, PathBuf};
use std::sync::Arc;
use anyhow::{Result, anyhow};
use tokio::fs;
use tokio::sync::mpsc;
use dashmap::DashMap;
use walkdir::WalkDir;
use notify::{Watcher, RecursiveMode, Event, EventKind};
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
use chrono::{DateTime, Utc};
use serde::{Serialize, Deserialize};
use uuid::Uuid;
use tree_sitter::{Parser, Language};

use crate::embeddings::{create_embedder, EmbeddingProvider};
use crate::search::vamana::VamanaIndex;
use crate::storage::StorageEngine;
use crate::security::{SecurityManager, UserId, DocumentId};

/// Supported file types for indexing
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FileType {
    Rust,
    Python,
    JavaScript,
    TypeScript,
    Java,
    Cpp,
    Go,
    Markdown,
    Toml,
    Json,
    Yaml,
    Text,
    Unknown,
}

impl FileType {
    fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "rs" => FileType::Rust,
            "py" => FileType::Python,
            "js" => FileType::JavaScript,
            "ts" | "tsx" => FileType::TypeScript,
            "java" => FileType::Java,
            "cpp" | "cc" | "cxx" | "hpp" | "h" => FileType::Cpp,
            "go" => FileType::Go,
            "md" => FileType::Markdown,
            "toml" => FileType::Toml,
            "json" => FileType::Json,
            "yaml" | "yml" => FileType::Yaml,
            "txt" => FileType::Text,
            _ => FileType::Unknown,
        }
    }
    
    fn get_language(&self) -> Option<Language> {
        match self {
            FileType::Rust => Some(tree_sitter_rust::language()),
            FileType::Python => Some(tree_sitter_python::language()),
            FileType::JavaScript => Some(tree_sitter_javascript::language()),
            FileType::Java => Some(tree_sitter_java::language()),
            FileType::Cpp => Some(tree_sitter_cpp::language()),
            _ => None,
        }
    }
}

/// Document chunk with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub id: Uuid,
    pub file_path: PathBuf,
    pub file_type: FileType,
    pub content: String,
    pub start_line: usize,
    pub end_line: usize,
    pub chunk_type: ChunkType,
    pub language: Option<String>,
    pub symbols: Vec<String>,
    pub imports: Vec<String>,
    pub hash: String,
    pub indexed_at: DateTime<Utc>,
    pub embedding: Option<Vec<f32>>,
    pub vector_id: Option<u32>,
}

/// Type of chunk
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ChunkType {
    Function,
    Class,
    Module,
    Documentation,
    Import,
    Test,
    Configuration,
    General,
}

/// Indexing statistics
#[derive(Debug, Clone)]
pub struct IndexStats {
    pub total_files: usize,
    pub total_chunks: usize,
    pub total_bytes: usize,
    pub file_types: DashMap<FileType, usize>,
    pub errors: Vec<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
}

/// Serializable version of IndexStats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStatsSnapshot {
    pub total_files: usize,
    pub total_chunks: usize,
    pub total_bytes: usize,
    pub file_types: std::collections::HashMap<FileType, usize>,
    pub errors: Vec<String>,
    pub start_time: DateTime<Utc>,
    pub end_time: Option<DateTime<Utc>>,
}

impl IndexStats {
    /// Convert to serializable snapshot
    pub fn snapshot(&self) -> IndexStatsSnapshot {
        let file_types: std::collections::HashMap<FileType, usize> = 
            self.file_types.iter().map(|entry| (*entry.key(), *entry.value())).collect();
            
        IndexStatsSnapshot {
            total_files: self.total_files,
            total_chunks: self.total_chunks,
            total_bytes: self.total_bytes,
            file_types,
            errors: self.errors.clone(),
            start_time: self.start_time,
            end_time: self.end_time,
        }
    }
}

/// Main file indexer
pub struct FileIndexer {
    storage: Arc<StorageEngine>,
    embedder: Arc<Box<dyn EmbeddingProvider>>,
    index: Arc<VamanaIndex>,
    security: Arc<SecurityManager>,
    chunks: Arc<DashMap<Uuid, DocumentChunk>>,
    file_cache: Arc<DashMap<PathBuf, Vec<Uuid>>>,
    vector_id_map: Arc<DashMap<u32, Uuid>>,
    stats: Arc<IndexStats>,
    config: IndexerConfig,
}

/// Indexer configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexerConfig {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub max_file_size: usize,
    pub ignore_patterns: Vec<String>,
    pub include_hidden: bool,
    pub watch_changes: bool,
    pub parallel_workers: usize,
}

impl Default for IndexerConfig {
    fn default() -> Self {
        Self {
            chunk_size: 500,  // lines
            chunk_overlap: 50,
            max_file_size: 10 * 1024 * 1024, // 10MB
            ignore_patterns: vec![
                "target".to_string(),
                "node_modules".to_string(),
                ".git".to_string(),
                "dist".to_string(),
                "build".to_string(),
                "__pycache__".to_string(),
                "*.pyc".to_string(),
                "*.pyo".to_string(),
                "*.exe".to_string(),
                "*.dll".to_string(),
                "*.so".to_string(),
            ],
            include_hidden: false,
            watch_changes: true,
            parallel_workers: num_cpus::get(),
        }
    }
}

impl FileIndexer {
    /// Create new file indexer
    pub async fn new(
        storage: Arc<StorageEngine>,
        security: Arc<SecurityManager>,
        config: IndexerConfig,
    ) -> Result<Self> {
        let embedder = Arc::new(create_embedder("native").await?);
        let index = Arc::new(VamanaIndex::new(
            64,   // R: out-degree
            100,  // L: search list size  
            1.2,  // alpha: pruning parameter
            crate::search::vamana::DistanceFunction::Cosine,
            true, // normalize vectors
            false // use scalar quantization
        ));
        
        Ok(Self {
            storage,
            embedder,
            index,
            security,
            chunks: Arc::new(DashMap::new()),
            file_cache: Arc::new(DashMap::new()),
            vector_id_map: Arc::new(DashMap::new()),
            stats: Arc::new(IndexStats {
                total_files: 0,
                total_chunks: 0,
                total_bytes: 0,
                file_types: DashMap::new(),
                errors: Vec::new(),
                start_time: Utc::now(),
                end_time: None,
            }),
            config,
        })
    }
    
    /// Index a directory recursively
    pub async fn index_directory(&self, path: &Path) -> Result<IndexStats> {
        println!("🚀 Starting indexing of: {}", path.display());
        println!("   Configuration:");
        println!("   - Chunk size: {} lines", self.config.chunk_size);
        println!("   - Workers: {}", self.config.parallel_workers);
        println!("   - Watch changes: {}", self.config.watch_changes);
        
        // Collect all files to index
        let files = self.collect_files(path)?;
        println!("\n📁 Found {} files to index", files.len());
        
        // Create processing channels - one per worker
        let mut workers = Vec::new();
        let mut senders = Vec::new();
        
        for i in 0..self.config.parallel_workers {
            let (tx, rx) = mpsc::channel::<PathBuf>(100);
            senders.push(tx);
            
            let indexer = self.clone_for_worker();
            
            workers.push(tokio::spawn(async move {
                indexer.worker_process(i, rx).await
            }));
        }
        
        // Send files to workers in round-robin fashion
        tokio::spawn(async move {
            for (idx, file) in files.iter().enumerate() {
                let worker_idx = idx % senders.len();
                if let Err(_) = senders[worker_idx].send(file.clone()).await {
                    break;
                }
            }
            
            // Close all senders to signal workers to finish
            drop(senders);
        });
        
        // Start file watcher if enabled
        if self.config.watch_changes {
            self.start_file_watcher(path).await?;
        }
        
        // Wait for all workers to complete
        for worker in workers {
            worker.await?;
        }
        
        // Finalize stats
        let mut stats = (*self.stats).clone();
        stats.end_time = Some(Utc::now());
        
        println!("\n✅ Indexing complete!");
        println!("   - Files: {}", stats.total_files);
        println!("   - Chunks: {}", stats.total_chunks);
        println!("   - Size: {:.2} MB", stats.total_bytes as f64 / 1_048_576.0);
        
        if !stats.errors.is_empty() {
            println!("\n⚠️  {} errors occurred:", stats.errors.len());
            for (i, error) in stats.errors.iter().take(5).enumerate() {
                println!("   {}. {}", i + 1, error);
            }
        }
        
        Ok(stats)
    }
    
    /// Collect all files to index
    fn collect_files(&self, path: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        
        for entry in WalkDir::new(path)
            .follow_links(true)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            
            // Skip directories
            if path.is_dir() {
                continue;
            }
            
            // Check ignore patterns
            if self.should_ignore(path) {
                continue;
            }
            
            // Check file size
            if let Ok(metadata) = path.metadata() {
                if metadata.len() > self.config.max_file_size as u64 {
                    continue;
                }
            }
            
            files.push(path.to_path_buf());
        }
        
        Ok(files)
    }
    
    /// Check if file should be ignored
    fn should_ignore(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        
        for pattern in &self.config.ignore_patterns {
            if path_str.contains(pattern) {
                return true;
            }
        }
        
        // Check hidden files
        if !self.config.include_hidden {
            for component in path.components() {
                if let Some(name) = component.as_os_str().to_str() {
                    if name.starts_with('.') && name != "." && name != ".." {
                        return true;
                    }
                }
            }
        }
        
        false
    }
    
    /// Worker process for parallel indexing
    async fn worker_process(&self, worker_id: usize, mut rx: mpsc::Receiver<PathBuf>) {
        println!("🔧 Worker {} started", worker_id);
        
        while let Some(file_path) = rx.recv().await {
            if let Err(e) = self.index_file(&file_path).await {
                eprintln!("❌ Worker {} error indexing {:?}: {}", 
                         worker_id, file_path, e);
                // Store error in stats
                // self.stats.errors.push(format!("{:?}: {}", file_path, e));
            }
        }
        
        println!("✅ Worker {} finished", worker_id);
    }
    
    /// Index a single file
    pub async fn index_file(&self, path: &Path) -> Result<()> {
        // Read file content
        let content = fs::read_to_string(path).await?;
        
        // Detect file type
        let file_type = path.extension()
            .and_then(|ext| ext.to_str())
            .map(FileType::from_extension)
            .unwrap_or(FileType::Unknown);
        
        // Update stats
        self.stats.file_types.entry(file_type).or_insert(0).add_assign(1);
        
        // Create chunks
        let chunks = self.create_chunks(path, &content, file_type).await?;
        
        // Generate embeddings and store
        for mut chunk in chunks {
            // Generate embedding
            let embedding = self.embedder.embed(&chunk.content).await
                .map_err(|e| anyhow!("Embedding error: {}", e))?;
            chunk.embedding = Some(embedding.clone());
            
            // Add to Vamana index
            let vector_id = self.index.insert(embedding);
            chunk.vector_id = Some(vector_id);
            
            // Store vector ID mapping
            self.vector_id_map.insert(vector_id, chunk.id);
            
            // Store in cache
            self.chunks.insert(chunk.id, chunk.clone());
            
            // Update file cache
            self.file_cache
                .entry(path.to_path_buf())
                .or_insert_with(Vec::new)
                .push(chunk.id);
        }
        
        println!("   ✓ Indexed: {}", path.display());
        
        Ok(())
    }
    
    /// Create chunks from file content
    async fn create_chunks(
        &self,
        path: &Path,
        content: &str,
        file_type: FileType,
    ) -> Result<Vec<DocumentChunk>> {
        let mut chunks = Vec::new();
        
        // Try syntax-aware chunking for code files
        if let Some(language) = file_type.get_language() {
            chunks = self.create_code_chunks(path, content, file_type, language)?;
        }
        
        // Fall back to line-based chunking if needed
        if chunks.is_empty() {
            chunks = self.create_line_chunks(path, content, file_type)?;
        }
        
        Ok(chunks)
    }
    
    /// Create syntax-aware chunks for code
    fn create_code_chunks(
        &self,
        path: &Path,
        content: &str,
        file_type: FileType,
        language: Language,
    ) -> Result<Vec<DocumentChunk>> {
        let mut parser = Parser::new();
        parser.set_language(language)?;
        
        let tree = parser.parse(content, None)
            .ok_or_else(|| anyhow!("Failed to parse file"))?;
        
        let mut chunks = Vec::new();
        let mut cursor = tree.walk();
        
        // Extract functions, classes, and other important nodes
        self.visit_node(&mut cursor, content, path, file_type, &mut chunks);
        
        Ok(chunks)
    }
    
    /// Visit tree-sitter nodes recursively
    fn visit_node(
        &self,
        cursor: &mut tree_sitter::TreeCursor,
        content: &str,
        path: &Path,
        file_type: FileType,
        chunks: &mut Vec<DocumentChunk>,
    ) {
        let node = cursor.node();
        let node_kind = node.kind();
        
        // Identify important nodes
        let chunk_type = match node_kind {
            "function_definition" | "function_item" | "method_definition" => Some(ChunkType::Function),
            "class_definition" | "struct_item" | "impl_item" => Some(ChunkType::Class),
            "module" | "mod_item" => Some(ChunkType::Module),
            "test" | "test_item" => Some(ChunkType::Test),
            _ => None,
        };
        
        if let Some(chunk_type) = chunk_type {
            let start_byte = node.start_byte();
            let end_byte = node.end_byte();
            let chunk_content = &content[start_byte..end_byte];
            
            let chunk = DocumentChunk {
                id: Uuid::new_v4(),
                file_path: path.to_path_buf(),
                file_type,
                content: chunk_content.to_string(),
                start_line: content[..start_byte].lines().count(),
                end_line: content[..end_byte].lines().count(),
                chunk_type,
                language: Some(file_type.to_string()),
                symbols: self.extract_symbols(chunk_content),
                imports: self.extract_imports(chunk_content),
                hash: self.compute_hash(chunk_content),
                indexed_at: Utc::now(),
                embedding: None,
                vector_id: None,
            };
            
            chunks.push(chunk);
        }
        
        // Visit children
        if cursor.goto_first_child() {
            loop {
                self.visit_node(cursor, content, path, file_type, chunks);
                if !cursor.goto_next_sibling() {
                    break;
                }
            }
            cursor.goto_parent();
        }
    }
    
    /// Create line-based chunks
    fn create_line_chunks(
        &self,
        path: &Path,
        content: &str,
        file_type: FileType,
    ) -> Result<Vec<DocumentChunk>> {
        let lines: Vec<&str> = content.lines().collect();
        let mut chunks = Vec::new();
        
        let chunk_size = self.config.chunk_size;
        let overlap = self.config.chunk_overlap;
        
        let mut start = 0;
        while start < lines.len() {
            let end = (start + chunk_size).min(lines.len());
            let chunk_lines = &lines[start..end];
            let chunk_content = chunk_lines.join("\n");
            
            let chunk = DocumentChunk {
                id: Uuid::new_v4(),
                file_path: path.to_path_buf(),
                file_type,
                content: chunk_content.clone(),
                start_line: start + 1,
                end_line: end,
                chunk_type: ChunkType::General,
                language: Some(file_type.to_string()),
                symbols: self.extract_symbols(&chunk_content),
                imports: self.extract_imports(&chunk_content),
                hash: self.compute_hash(&chunk_content),
                indexed_at: Utc::now(),
                embedding: None,
                vector_id: None,
            };
            
            chunks.push(chunk);
            
            // Move to next chunk with overlap
            start += chunk_size - overlap;
        }
        
        Ok(chunks)
    }
    
    /// Extract symbols from code
    fn extract_symbols(&self, content: &str) -> Vec<String> {
        let mut symbols = Vec::new();
        
        // Simple regex-based extraction (can be improved)
        let re = regex::Regex::new(r"\b(fn|struct|impl|class|def|function|const|let|var)\s+(\w+)")
            .unwrap();
        
        for cap in re.captures_iter(content) {
            if let Some(symbol) = cap.get(2) {
                symbols.push(symbol.as_str().to_string());
            }
        }
        
        symbols
    }
    
    /// Extract imports from code
    fn extract_imports(&self, content: &str) -> Vec<String> {
        let mut imports = Vec::new();
        
        // Extract use/import statements
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("use ") || 
               trimmed.starts_with("import ") ||
               trimmed.starts_with("from ") ||
               trimmed.starts_with("#include") {
                imports.push(trimmed.to_string());
            }
        }
        
        imports
    }
    
    /// Compute content hash
    fn compute_hash(&self, content: &str) -> String {
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
    
    /// Start file watcher for real-time updates
    async fn start_file_watcher(&self, path: &Path) -> Result<()> {
        let (tx, mut rx) = mpsc::channel(100);
        let indexer = self.clone_for_worker();
        let watch_path = path.to_path_buf();
        
        // Spawn watcher thread
        std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(async {
                let mut debouncer = new_debouncer(
                    std::time::Duration::from_secs(2),
                    move |result: Result<Vec<DebouncedEvent>, notify::Error>| {
                        if let Ok(events) = result {
                            for event in events {
                                let _ = tx.blocking_send(event);
                            }
                        }
                    }
                ).unwrap();
                
                debouncer.watcher()
                    .watch(&watch_path, RecursiveMode::Recursive)
                    .unwrap();
                
                // Keep watcher alive
                loop {
                    std::thread::sleep(std::time::Duration::from_secs(60));
                }
            });
        });
        
        // Process file change events
        tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                // For now, handle all events the same way
                // TODO: Differentiate between create/modify/delete when API is stable
                println!("📝 File event: {:?}", event.path);
                if let Err(e) = indexer.index_file(&event.path).await {
                    eprintln!("Error re-indexing file: {}", e);
                }
            }
        });
        
        println!("👁️  File watcher started for: {}", path.display());
        
        Ok(())
    }
    
    /// Remove file from index
    async fn remove_file(&self, path: &Path) {
        if let Some((_, chunk_ids)) = self.file_cache.remove(path) {
            for chunk_id in chunk_ids {
                self.chunks.remove(&chunk_id);
                // Also remove from HNSW index
                // self.index.remove(chunk_id.as_bytes());
            }
        }
    }
    
    /// Search for documents
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<DocumentChunk>> {
        // Generate query embedding
        let query_embedding = self.embedder.embed(query).await
            .map_err(|e| anyhow!("Query embedding error: {}", e))?;
        
        // Search in Vamana index
        let results = self.index.search(&query_embedding, limit, None);
        
        // Retrieve chunks by vector ID
        let mut chunks = Vec::new();
        for result in results {
            if let Some(chunk_id) = self.vector_id_map.get(&result.id) {
                if let Some(chunk) = self.chunks.get(&chunk_id) {
                    chunks.push(chunk.clone());
                }
            }
        }
        
        Ok(chunks)
    }
    
    /// Clone for worker thread
    fn clone_for_worker(&self) -> Self {
        Self {
            storage: self.storage.clone(),
            embedder: self.embedder.clone(),
            index: self.index.clone(),
            security: self.security.clone(),
            chunks: self.chunks.clone(),
            file_cache: self.file_cache.clone(),
            vector_id_map: self.vector_id_map.clone(),
            stats: self.stats.clone(),
            config: self.config.clone(),
        }
    }
}

impl FileType {
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
}

use std::ops::AddAssign;

// AddAssign is already implemented for usize in std