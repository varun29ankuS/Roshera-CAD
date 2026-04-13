/// Embedding Provider for TurboRAG
/// 
/// Provides both trait interface and implementation modules

use async_trait::async_trait;
use anyhow::{Result, anyhow};
use std::sync::Arc;

pub mod implementation;

// Re-export the trait for compatibility
pub use implementation::{EmbeddingService, EmbeddingConfig, DocumentType};

/// Embedding provider trait (for backward compatibility)
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>>;
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>>;
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
    async fn embed(&self, text: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        // Detect document type
        let doc_type = EmbeddingService::detect_type(text, None);
        
        // Generate embeddings
        self.service.embed(text, doc_type)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }
    
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
        // Detect document type from first text
        let doc_type = if !texts.is_empty() {
            EmbeddingService::detect_type(&texts[0], None)
        } else {
            DocumentType::Text
        };
        
        self.service.embed_batch(texts, doc_type)
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error>)
    }
    
    fn dimension(&self) -> usize {
        self.dimension
    }
}

/// Create an embedding provider based on configuration
pub async fn create_embedder(provider_type: &str) -> Result<Box<dyn EmbeddingProvider>> {
    match provider_type {
        "bge" | "onnx" | "production" => {
            let adapter = EmbeddingAdapter::new().await?;
            Ok(Box::new(adapter))
        },
        "mock" | "test" => {
            // Keep mock for testing
            Ok(Box::new(MockEmbedder::new(1024)))
        },
        _ => {
            // Default to BGE
            let adapter = EmbeddingAdapter::new().await?;
            Ok(Box::new(adapter))
        }
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
}

#[async_trait]
impl EmbeddingProvider for MockEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
        // Generate deterministic embeddings based on text hash
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        
        let mut hasher = DefaultHasher::new();
        text.hash(&mut hasher);
        let hash = hasher.finish();
        
        // Generate pseudo-random but deterministic vector
        let mut rng = rand::rngs::StdRng::seed_from_u64(hash);
        use rand::Rng;
        
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
    
    async fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, Box<dyn std::error::Error>> {
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