/// TurboRAG Server - Simple, fast RAG that actually works
/// 
/// NO distributed systems, NO learning algorithms, NO fantasy
/// Just: Index → Search → Return results

use axum::Router;
use std::sync::Arc;
use tokio::sync::RwLock;
use anyhow::Result;

mod api;
mod search;
mod indexer;
mod chunker;
mod embeddings;

use api::{ApiState, create_router};
use search::vamana::{VamanaIndex, DistanceFunction};
use search::InvertedIndex;
use indexer::DocumentIndexer;
use chunker::IntelligentChunker;
use embeddings::create_embedder;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    tracing_subscriber::fmt::init();
    
    println!("🚀 TurboRAG Server Starting...\n");
    println!("   Using: Vamana/DiskANN (not broken HNSW)");
    println!("   Dimension: 1536 (OpenAI compatible)");
    println!("   Compression: 4x with Scalar Quantization\n");
    
    // Create Vamana index with proper configuration
    let vector_index = Arc::new(VamanaIndex::new(
        64,                             // R: out-degree for 1536-dim
        100,                            // L: search list size
        1.2,                            // alpha: robust pruning
        DistanceFunction::Cosine,      // for normalized embeddings
        true,                           // normalize vectors
        true,                           // use scalar quantization
    ));
    
    // Create text index for hybrid search
    let text_index = Arc::new(RwLock::new(InvertedIndex::new()));
    
    // Create document storage (in-memory for now)
    let documents = Arc::new(RwLock::new(Vec::new()));
    let chunks = Arc::new(RwLock::new(Vec::new()));
    
    // Create embedder (mock for now, replace with OpenAI)
    let embedder = create_embedder("mock");
    
    // Create chunker
    let chunker = Arc::new(IntelligentChunker::new());
    
    // Create API state
    let state = Arc::new(ApiState {
        vector_index,
        text_index,
        documents,
        chunks,
        embedder,
        chunker,
    });
    
    // Create router
    let app = create_router(state);
    
    // Start server
    let addr = "0.0.0.0:3001";
    println!("✅ Server running at http://{}", addr);
    println!("📊 Dashboard at http://{}/", addr);
    println!("🔧 API endpoints:");
    println!("   POST /api/index - Index documents");
    println!("   POST /api/search - Search documents");
    println!("   POST /api/chat - Chat with RAG");
    println!("   GET  /api/stats - Get statistics\n");
    
    axum::Server::bind(&addr.parse()?)
        .serve(app.into_make_service())
        .await?;
    
    Ok(())
}