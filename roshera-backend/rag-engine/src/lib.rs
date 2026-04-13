//! TurboWit RAG Engine - Zero-dependency, distributed RAG system for Roshera CAD
//!
//! This crate provides a complete Retrieval-Augmented Generation system that:
//! - Indexes code, CAD files, and user sessions
//! - Provides distributed search with no external dependencies
//! - Learns from user behavior and improves continuously
//! - Personalizes responses based on user expertise

#![warn(missing_docs)]
#![forbid(unsafe_code)]

pub mod api;
pub mod cache;
pub mod distribution;
pub mod embeddings;
pub mod indexer;
pub mod ingestion;
pub mod intelligence;
pub mod learning;
pub mod monitoring;
pub mod query;
pub mod search;
pub mod security;
pub mod splits;
pub mod storage;

// Re-export main types
pub use cache::{LayeredCache, TurboCache};
pub use distribution::{DistributionLayer, TurboRaft};
pub use ingestion::{IngestionPipeline, CodeWatcher};
pub use intelligence::{IntelligenceEngine, UserLearning};
pub use learning::{ContinuousLearning, EdgeCaseDetector};
pub use query::{QueryExecutor, RAGResponse};
pub use search::{TurboSearch, SearchResult};
pub use splits::{Split, SplitManager};
pub use storage::{StorageEngine, TurboStorage};

use std::path::Path;
use std::sync::Arc;
use anyhow::Result;

/// Main RAG engine that coordinates all components
pub struct TurboWitRAG {
    ingestion: IngestionPipeline,
    storage: Arc<StorageEngine>,
    intelligence: Arc<IntelligenceEngine>,
    distribution: Arc<DistributionLayer>,
    query_executor: QueryExecutor,
    learning: ContinuousLearning,
}

impl TurboWitRAG {
    /// Create a new RAG engine instance
    pub async fn new(config: RAGConfig) -> Result<Self> {
        let storage = Arc::new(StorageEngine::new(&config.storage_path).await?);
        let intelligence = Arc::new(IntelligenceEngine::new(&config.intelligence_config)?);
        let distribution = Arc::new(DistributionLayer::new(&config.distribution_config).await?);
        let ingestion = IngestionPipeline::new(storage.clone());
        let query_executor = QueryExecutor::new(
            storage.clone(),
            intelligence.clone(),
            distribution.clone(),
        );
        let learning = ContinuousLearning::new(intelligence.clone());

        Ok(Self {
            ingestion,
            storage,
            intelligence,
            distribution,
            query_executor,
            learning,
        })
    }

    /// Index a codebase directory
    pub async fn index_codebase(&self, path: &Path) -> Result<()> {
        self.ingestion.index_directory(path).await
    }

    /// Execute a search query
    pub async fn search(&self, query: &str, user_id: uuid::Uuid) -> Result<RAGResponse> {
        self.query_executor.execute(query, user_id).await
    }

    /// Learn from a user session
    pub async fn learn_from_session(&mut self, session: Session) -> Result<()> {
        self.learning.learn_from_session(session).await
    }
}

/// Configuration for the RAG engine
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RAGConfig {
    /// Path for storage files
    pub storage_path: std::path::PathBuf,
    
    /// Intelligence engine configuration
    pub intelligence_config: IntelligenceConfig,
    
    /// Distribution configuration
    pub distribution_config: DistributionConfig,
    
    /// Cache configuration
    pub cache_config: CacheConfig,
}

impl Default for RAGConfig {
    fn default() -> Self {
        Self {
            storage_path: std::path::PathBuf::from("./rag_data"),
            intelligence_config: IntelligenceConfig::default(),
            distribution_config: DistributionConfig::default(),
            cache_config: CacheConfig::default(),
        }
    }
}

/// Intelligence engine configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IntelligenceConfig {
    /// Enable user learning
    pub enable_user_learning: bool,
    
    /// Enable team knowledge sharing
    pub enable_team_knowledge: bool,
    
    /// Enable continuous improvement
    pub enable_continuous_improvement: bool,
}

impl Default for IntelligenceConfig {
    fn default() -> Self {
        Self {
            enable_user_learning: true,
            enable_team_knowledge: true,
            enable_continuous_improvement: true,
        }
    }
}

/// Distribution configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DistributionConfig {
    /// Node ID for this instance
    pub node_id: String,
    
    /// Peer nodes for distribution
    pub peers: Vec<String>,
    
    /// Replication factor
    pub replication_factor: usize,
}

impl Default for DistributionConfig {
    fn default() -> Self {
        Self {
            node_id: uuid::Uuid::new_v4().to_string(),
            peers: vec![],
            replication_factor: 2,
        }
    }
}

/// Cache configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheConfig {
    /// Maximum cache size in MB
    pub max_size_mb: usize,
    
    /// TTL for cache entries in seconds
    pub ttl_seconds: u64,
    
    /// Enable distributed caching
    pub enable_distributed: bool,
}

impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            max_size_mb: 1024,
            ttl_seconds: 3600,
            enable_distributed: false,
        }
    }
}

/// Represents a user session for learning
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Session {
    /// User ID
    pub user_id: uuid::Uuid,
    
    /// Session operations
    pub operations: Vec<Operation>,
    
    /// Session duration
    pub duration: std::time::Duration,
    
    /// Whether session was successful
    pub success: bool,
    
    /// Any errors encountered
    pub errors: Option<Vec<String>>,
}

/// Represents an operation in a session
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Operation {
    /// Operation type
    pub op_type: String,
    
    /// Operation parameters
    pub params: serde_json::Value,
    
    /// Timestamp
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rag_creation() {
        let config = RAGConfig::default();
        let rag = TurboWitRAG::new(config).await;
        assert!(rag.is_ok());
    }
}
