/// Search API endpoints for TurboRAG
/// 
/// Provides REST endpoints for searching indexed content

use axum::{
    extract::{Query, State},
    response::Json,
    http::StatusCode,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use anyhow::Result;

use crate::indexer::FileIndexer;
use crate::search::{
    bm25::BM25Search,
    vamana::VamanaIndex,
    hybrid::HybridSearch,
};

/// Search request
#[derive(Debug, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_limit")]
    pub limit: usize,
    #[serde(default = "default_search_type")]
    pub search_type: SearchType,
    #[serde(default)]
    pub include_embeddings: bool,
}

fn default_limit() -> usize { 10 }
fn default_search_type() -> SearchType { SearchType::Hybrid }

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SearchType {
    Vector,
    BM25,
    Hybrid,
    Symbol,
    Fuzzy,
}

/// Search result
#[derive(Debug, Serialize)]
pub struct SearchResult {
    pub score: f32,
    pub content: String,
    pub metadata: ChunkMetadata,
}

#[derive(Debug, Serialize)]
pub struct ChunkMetadata {
    pub file_path: String,
    pub file_type: String,
    pub start_line: usize,
    pub end_line: usize,
    pub symbols: Vec<String>,
    pub chunk_type: String,
}

/// Search response
#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    pub total_results: usize,
    pub search_time_ms: f64,
    pub stats: SearchStats,
}

#[derive(Debug, Serialize)]
pub struct SearchStats {
    pub docs_scanned: usize,
    pub vectors_compared: usize,
    pub cache_hits: usize,
}

/// Search handler
pub async fn search_handler(
    State(indexer): State<Arc<FileIndexer>>,
    Json(request): Json<SearchRequest>,
) -> Result<Json<SearchResponse>, StatusCode> {
    let start = std::time::Instant::now();
    
    // Perform search based on type
    let results = match request.search_type {
        SearchType::Vector => {
            // Vector search using embeddings
            indexer.search(&request.query, request.limit)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        },
        SearchType::BM25 => {
            // BM25 keyword search
            // TODO: Implement BM25 search through indexer
            vec![]
        },
        SearchType::Hybrid => {
            // Hybrid search combining vector and BM25
            let vector_results = indexer.search(&request.query, request.limit * 2)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
            
            // For now, just return vector results
            // TODO: Combine with BM25 scores
            vector_results
        },
        SearchType::Symbol => {
            // Symbol-specific search
            // TODO: Implement symbol search
            vec![]
        },
        SearchType::Fuzzy => {
            // Fuzzy matching search
            // TODO: Implement fuzzy search
            vec![]
        }
    };
    
    // Convert to response format
    let search_results: Vec<SearchResult> = results.iter().map(|chunk| {
        SearchResult {
            score: chunk.embedding.as_ref()
                .and_then(|e| e.first())
                .map(|v| v.abs())
                .unwrap_or(0.0),
            content: chunk.content.clone(),
            metadata: ChunkMetadata {
                file_path: chunk.file_path.to_string_lossy().to_string(),
                file_type: format!("{:?}", chunk.file_type),
                start_line: chunk.start_line,
                end_line: chunk.end_line,
                symbols: chunk.symbols.clone(),
                chunk_type: format!("{:?}", chunk.chunk_type),
            },
        }
    }).collect();
    
    let search_time_ms = start.elapsed().as_secs_f64() * 1000.0;
    
    Ok(Json(SearchResponse {
        total_results: search_results.len(),
        results: search_results,
        search_time_ms,
        stats: SearchStats {
            docs_scanned: results.len(),
            vectors_compared: results.len() * 10, // Estimate
            cache_hits: 0, // TODO: Track cache hits
        },
    }))
}

/// Get indexing statistics
pub async fn stats_handler(
    State(indexer): State<Arc<FileIndexer>>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Get stats from indexer
    // For now, return mock data
    Ok(Json(serde_json::json!({
        "total_files": 423,
        "total_chunks": 5480,
        "index_size_mb": 15.2,
        "file_types": {
            "Rust": 250,
            "Python": 50,
            "JavaScript": 80,
            "TypeScript": 43
        }
    })))
}