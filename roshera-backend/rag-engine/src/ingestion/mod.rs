//! Ingestion pipeline for indexing code, CAD files, and sessions
//!
//! Handles:
//! - Code parsing and AST extraction
//! - CAD file analysis
//! - Timeline event processing
//! - Real-time file watching

use notify::{Event, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc;
use syn::{visit::Visit, File as SynFile, Item};

use crate::storage::{Document, DocumentMetadata, StorageEngine};
use crate::search::TurboSearch;

/// Main ingestion pipeline
pub struct IngestionPipeline {
    storage: Arc<StorageEngine>,
    code_parser: Arc<CodeParser>,
    cad_analyzer: Arc<CADAnalyzer>,
    timeline_processor: Arc<TimelineProcessor>,
    file_watcher: Option<CodeWatcher>,
}

/// Code file watcher for real-time updates
pub struct CodeWatcher {
    watcher: notify::RecommendedWatcher,
    rx: mpsc::UnboundedReceiver<WatchEvent>,
}

/// Watch event
#[derive(Debug, Clone)]
pub struct WatchEvent {
    pub path: PathBuf,
    pub event_type: WatchEventType,
}

/// Watch event type
#[derive(Debug, Clone)]
pub enum WatchEventType {
    Created,
    Modified,
    Deleted,
    Renamed(PathBuf),
}

/// Code parser for extracting symbols and dependencies
pub struct CodeParser {
    extractors: Vec<Box<dyn CodeExtractor>>,
}

/// Trait for code extraction
pub trait CodeExtractor: Send + Sync {
    fn extract(&self, content: &str) -> ExtractedInfo;
}

/// Extracted code information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ExtractedInfo {
    pub symbols: Vec<Symbol>,
    pub imports: Vec<Import>,
    pub comments: Vec<Comment>,
    pub tests: Vec<Test>,
}

/// Code symbol
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Symbol {
    pub name: String,
    pub kind: SymbolKind,
    pub line: usize,
    pub column: usize,
    pub signature: Option<String>,
}

/// Symbol kind
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SymbolKind {
    Function,
    Struct,
    Enum,
    Trait,
    Impl,
    Module,
    Const,
    Type,
}

/// Import statement
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Import {
    pub module: String,
    pub items: Vec<String>,
    pub alias: Option<String>,
}

/// Code comment
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Comment {
    pub text: String,
    pub line: usize,
    pub is_doc: bool,
}

/// Test definition
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Test {
    pub name: String,
    pub line: usize,
    pub is_ignored: bool,
}

/// CAD file analyzer
pub struct CADAnalyzer {
    geometry_extractor: GeometryExtractor,
    feature_detector: FeatureDetector,
}

/// Geometry extractor for CAD files
struct GeometryExtractor;

/// Feature detector for CAD models
struct FeatureDetector;

/// Timeline processor for operation history
pub struct TimelineProcessor {
    operation_analyzer: OperationAnalyzer,
    pattern_detector: PatternDetector,
}

/// Operation analyzer
struct OperationAnalyzer;

/// Pattern detector for common workflows
struct PatternDetector;

/// Rust code extractor
pub struct RustExtractor;

impl IngestionPipeline {
    /// Create new ingestion pipeline
    pub fn new(storage: Arc<StorageEngine>) -> Self {
        Self {
            storage,
            code_parser: Arc::new(CodeParser::new()),
            cad_analyzer: Arc::new(CADAnalyzer::new()),
            timeline_processor: Arc::new(TimelineProcessor::new()),
            file_watcher: None,
        }
    }

    /// Index a directory recursively
    pub async fn index_directory(&self, path: &Path) -> anyhow::Result<()> {
        let entries = self.walk_directory(path)?;
        
        for entry in entries {
            if let Ok(content) = tokio::fs::read_to_string(&entry).await {
                self.index_file(&entry, &content).await?;
            }
        }
        
        Ok(())
    }

    /// Index a single file
    pub async fn index_file(&self, path: &Path, content: &str) -> anyhow::Result<()> {
        let file_type = self.detect_file_type(path);
        
        let doc = match file_type {
            FileType::Rust => self.index_rust_file(path, content).await?,
            FileType::Python => self.index_python_file(path, content).await?,
            FileType::JavaScript => self.index_js_file(path, content).await?,
            FileType::CAD => self.index_cad_file(path, content).await?,
            FileType::Timeline => self.index_timeline_file(path, content).await?,
            FileType::Other => self.index_generic_file(path, content).await?,
        };
        
        self.storage.write(doc).await?;
        Ok(())
    }

    /// Start watching a directory for changes
    pub async fn watch_directory(&mut self, path: &Path) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::unbounded_channel();
        
        let mut watcher = notify::recommended_watcher(move |res: Result<Event, _>| {
            if let Ok(event) = res {
                let watch_event = match event.kind {
                    notify::EventKind::Create(_) => WatchEvent {
                        path: event.paths[0].clone(),
                        event_type: WatchEventType::Created,
                    },
                    notify::EventKind::Modify(_) => WatchEvent {
                        path: event.paths[0].clone(),
                        event_type: WatchEventType::Modified,
                    },
                    notify::EventKind::Remove(_) => WatchEvent {
                        path: event.paths[0].clone(),
                        event_type: WatchEventType::Deleted,
                    },
                    _ => return,
                };
                tx.send(watch_event).ok();
            }
        })?;
        
        watcher.watch(path, RecursiveMode::Recursive)?;
        
        self.file_watcher = Some(CodeWatcher { watcher, rx });
        Ok(())
    }

    /// Process watch events
    pub async fn process_watch_events(&mut self) -> anyhow::Result<()> {
        // Collect events first to avoid borrow conflict
        let events: Vec<_> = if let Some(ref mut watcher) = self.file_watcher {
            let mut collected = Vec::new();
            while let Ok(event) = watcher.rx.try_recv() {
                collected.push(event);
            }
            collected
        } else {
            return Ok(());
        };

        // Now process events without holding a borrow on self.file_watcher
        for event in events {
            match event.event_type {
                WatchEventType::Created | WatchEventType::Modified => {
                    if let Ok(content) = tokio::fs::read_to_string(&event.path).await {
                        self.index_file(&event.path, &content).await?;
                    }
                }
                WatchEventType::Deleted => {
                    // Mark document as deleted in storage
                }
                WatchEventType::Renamed(_new_path) => {
                    // Update document path
                }
            }
        }
        Ok(())
    }

    fn walk_directory(&self, path: &Path) -> anyhow::Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        
        for entry in walkdir::WalkDir::new(path) {
            let entry = entry?;
            if entry.file_type().is_file() {
                files.push(entry.path().to_path_buf());
            }
        }
        
        Ok(files)
    }

    fn detect_file_type(&self, path: &Path) -> FileType {
        match path.extension().and_then(|s| s.to_str()) {
            Some("rs") => FileType::Rust,
            Some("py") => FileType::Python,
            Some("js") | Some("ts") => FileType::JavaScript,
            Some("ros") | Some("step") | Some("stl") => FileType::CAD,
            Some("timeline") => FileType::Timeline,
            _ => FileType::Other,
        }
    }

    async fn index_rust_file(&self, path: &Path, content: &str) -> anyhow::Result<Document> {
        let info = self.code_parser.parse_rust(content)?;
        
        let metadata = DocumentMetadata {
            source: path.to_string_lossy().to_string(),
            timestamp: chrono::Utc::now(),
            tags: vec!["rust".to_string(), "code".to_string()],
            checksum: blake3::hash(content.as_bytes()).as_bytes().clone(),
        };
        
        let doc_content = serde_json::to_vec(&info)?;
        
        Ok(Document {
            id: uuid::Uuid::new_v4(),
            content: doc_content,
            metadata,
            version: 1,
            deleted: false,
        })
    }

    async fn index_python_file(&self, path: &Path, content: &str) -> anyhow::Result<Document> {
        // Python parsing would be implemented here
        self.index_generic_file(path, content).await
    }

    async fn index_js_file(&self, path: &Path, content: &str) -> anyhow::Result<Document> {
        // JavaScript parsing would be implemented here
        self.index_generic_file(path, content).await
    }

    async fn index_cad_file(&self, path: &Path, content: &str) -> anyhow::Result<Document> {
        let analysis = self.cad_analyzer.analyze(content)?;
        
        let metadata = DocumentMetadata {
            source: path.to_string_lossy().to_string(),
            timestamp: chrono::Utc::now(),
            tags: vec!["cad".to_string(), "geometry".to_string()],
            checksum: blake3::hash(content.as_bytes()).as_bytes().clone(),
        };
        
        let doc_content = serde_json::to_vec(&analysis)?;
        
        Ok(Document {
            id: uuid::Uuid::new_v4(),
            content: doc_content,
            metadata,
            version: 1,
            deleted: false,
        })
    }

    async fn index_timeline_file(&self, path: &Path, content: &str) -> anyhow::Result<Document> {
        let analysis = self.timeline_processor.process(content)?;
        
        let metadata = DocumentMetadata {
            source: path.to_string_lossy().to_string(),
            timestamp: chrono::Utc::now(),
            tags: vec!["timeline".to_string(), "history".to_string()],
            checksum: blake3::hash(content.as_bytes()).as_bytes().clone(),
        };
        
        let doc_content = serde_json::to_vec(&analysis)?;
        
        Ok(Document {
            id: uuid::Uuid::new_v4(),
            content: doc_content,
            metadata,
            version: 1,
            deleted: false,
        })
    }

    async fn index_generic_file(&self, path: &Path, content: &str) -> anyhow::Result<Document> {
        let metadata = DocumentMetadata {
            source: path.to_string_lossy().to_string(),
            timestamp: chrono::Utc::now(),
            tags: vec!["generic".to_string()],
            checksum: blake3::hash(content.as_bytes()).as_bytes().clone(),
        };
        
        Ok(Document {
            id: uuid::Uuid::new_v4(),
            content: content.as_bytes().to_vec(),
            metadata,
            version: 1,
            deleted: false,
        })
    }
}

impl CodeParser {
    pub fn new() -> Self {
        Self {
            extractors: vec![Box::new(RustExtractor)],
        }
    }

    pub fn parse_rust(&self, content: &str) -> anyhow::Result<ExtractedInfo> {
        Ok(self.extractors[0].extract(content))
    }
}

impl CodeExtractor for RustExtractor {
    fn extract(&self, content: &str) -> ExtractedInfo {
        let mut info = ExtractedInfo {
            symbols: Vec::new(),
            imports: Vec::new(),
            comments: Vec::new(),
            tests: Vec::new(),
        };
        
        // Parse with syn
        if let Ok(file) = syn::parse_file(content) {
            let mut visitor = RustVisitor::new(&mut info);
            visitor.visit_file(&file);
        }
        
        info
    }
}

/// Visitor for Rust AST
struct RustVisitor<'a> {
    info: &'a mut ExtractedInfo,
}

impl<'a> RustVisitor<'a> {
    fn new(info: &'a mut ExtractedInfo) -> Self {
        Self { info }
    }
}

impl<'a> Visit<'_> for RustVisitor<'a> {
    fn visit_item_fn(&mut self, node: &syn::ItemFn) {
        self.info.symbols.push(Symbol {
            name: node.sig.ident.to_string(),
            kind: SymbolKind::Function,
            line: 0, // Would need span info
            column: 0,
            signature: Some(format!("{}", quote::quote!(#node.sig))),
        });
        
        // Check if it's a test
        for attr in &node.attrs {
            if attr.path().is_ident("test") {
                self.info.tests.push(Test {
                    name: node.sig.ident.to_string(),
                    line: 0,
                    is_ignored: false,
                });
            }
        }
    }
    
    fn visit_item_struct(&mut self, node: &syn::ItemStruct) {
        self.info.symbols.push(Symbol {
            name: node.ident.to_string(),
            kind: SymbolKind::Struct,
            line: 0,
            column: 0,
            signature: None,
        });
    }
    
    fn visit_item_use(&mut self, node: &syn::ItemUse) {
        let module = format!("{}", quote::quote!(#node.tree));
        self.info.imports.push(Import {
            module,
            items: Vec::new(),
            alias: None,
        });
    }
}

impl CADAnalyzer {
    pub fn new() -> Self {
        Self {
            geometry_extractor: GeometryExtractor,
            feature_detector: FeatureDetector,
        }
    }

    pub fn analyze(&self, content: &str) -> anyhow::Result<CADAnalysis> {
        Ok(CADAnalysis {
            geometry_count: 0,
            feature_count: 0,
            operations: Vec::new(),
        })
    }
}

impl TimelineProcessor {
    pub fn new() -> Self {
        Self {
            operation_analyzer: OperationAnalyzer,
            pattern_detector: PatternDetector,
        }
    }

    pub fn process(&self, content: &str) -> anyhow::Result<TimelineAnalysis> {
        Ok(TimelineAnalysis {
            operation_count: 0,
            patterns: Vec::new(),
        })
    }
}

/// File type enum
#[derive(Debug, Clone)]
enum FileType {
    Rust,
    Python,
    JavaScript,
    CAD,
    Timeline,
    Other,
}

/// CAD analysis result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CADAnalysis {
    geometry_count: usize,
    feature_count: usize,
    operations: Vec<String>,
}

/// Timeline analysis result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct TimelineAnalysis {
    operation_count: usize,
    patterns: Vec<String>,
}

// Add walkdir to dependencies
use walkdir;