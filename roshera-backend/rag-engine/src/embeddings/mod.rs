/// Embedding Provider for TurboRAG
/// 
/// Provides both trait interface and implementation modules

use async_trait::async_trait;
use anyhow::{Result, anyhow};
use std::sync::Arc;

pub mod implementation;
pub mod native;

// Re-export the main types
pub use implementation::{EmbeddingService, EmbeddingConfig, DocumentType};
pub use native::{NativeEmbeddingService, EMBEDDING_DIM};

/// Embedding provider trait (for backward compatibility)
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>>;
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>>;
    fn dimension(&self) -> usize;
}

/// Adapter to use new EmbeddingService with old trait
pub struct EmbeddingAdapter {
    service: Arc<EmbeddingService>,
    dimension: usize,
}

impl EmbeddingAdapter {
    pub async fn new() -> Result<Self> {
        let config = EmbeddingConfig::default();
        let service = Arc::new(EmbeddingService::new(config).await?);
        
        Ok(Self {
            service,
            dimension: 1024, // BGE-large dimension
        })
    }
    
    pub async fn with_config(config: EmbeddingConfig) -> Result<Self> {
        let service = Arc::new(EmbeddingService::new(config).await?);
        
        Ok(Self {
            service,
            dimension: 1024,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for EmbeddingAdapter {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        // Detect document type
        let doc_type = EmbeddingService::detect_type(text, None);
        
        // Generate embeddings
        self.service.embed(text, doc_type)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
    
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        // Detect document type from first text
        let doc_type = if !texts.is_empty() {
            EmbeddingService::detect_type(&texts[0], None)
        } else {
            DocumentType::Text
        };
        
        self.service.embed_batch(texts, doc_type)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
    
    fn dimension(&self) -> usize {
        self.dimension
    }
}

/// Create an embedding provider based on configuration
pub async fn create_embedder(provider_type: &str) -> Result<Box<dyn EmbeddingProvider>> {
    match provider_type {
        "native" | "production" => {
            // Use native Rust implementation - works on all platforms
            let adapter = NativeEmbeddingAdapter::new().await?;
            Ok(Box::new(adapter))
        },
        "bge" | "onnx" => {
            // Try to use ONNX implementation if available
            let adapter = EmbeddingAdapter::new().await?;
            Ok(Box::new(adapter))
        },
        "mock" | "test" => {
            // Keep mock for testing
            Ok(Box::new(MockEmbedder::new(EMBEDDING_DIM)))
        },
        _ => {
            // Default to native implementation
            let adapter = NativeEmbeddingAdapter::new().await?;
            Ok(Box::new(adapter))
        }
    }
}

/// Adapter for native embedding service
pub struct NativeEmbeddingAdapter {
    service: Arc<NativeEmbeddingService>,
}

impl NativeEmbeddingAdapter {
    pub async fn new() -> Result<Self> {
        let data_path = std::path::Path::new("./embeddings_data");
        tokio::fs::create_dir_all(&data_path).await?;
        let service = Arc::new(NativeEmbeddingService::new(data_path).await?);
        Ok(Self { service })
    }
}

#[async_trait]
impl EmbeddingProvider for NativeEmbeddingAdapter {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        self.service.embed(text)
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))
    }
    
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }
    
    fn dimension(&self) -> usize {
        EMBEDDING_DIM
    }
}

/// Mock embedding provider for testing
pub struct MockEmbedder {
    dimension: usize,
}

impl MockEmbedder {
    pub fn new(dimension: usize) -> Self {
        Self { dimension }
    }
    
    pub fn new_default() -> Self {
        Self { dimension: EMBEDDING_DIM }
    }
}

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    async fn embed(&self, text: &str) -> anyhow::Result<Vec<f32>> {
        // Generate deterministic embeddings based on text hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();
        
        // Generate pseudo-random but deterministic vector
        use rand::rngs::StdRng;
        use rand::{Rng, SeedableRng};
        let mut rng = StdRng::seed_from_u64(hash);
        
        let mut embedding = Vec::with_capacity(self.dimension);
        for _ in 0..self.dimension {
            embedding.push(rng.gen_range(-1.0..1.0));
        }
        
        // Normalize to unit vector
        let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in &mut embedding {
                *val /= norm;
            }
        }
        
        Ok(embedding)
    }
    
    async fn embed_batch(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut embeddings = Vec::new();
        for text in texts {
            embeddings.push(self.embed(text).await?);
        }
        Ok(embeddings)
    }
    
    fn dimension(&self) -> usize {
        self.dimension
    }
}