/// BM25 Text Search Implementation
/// 
/// Provides high-performance keyword search using the BM25 algorithm,
/// which is the gold standard for text retrieval in search engines.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use dashmap::DashMap;
use serde::{Serialize, Deserialize};
use anyhow::{Result, anyhow};

/// BM25 parameters (tuned for code search)
#[derive(Debug, Clone, Copy)]
pub struct BM25Params {
    /// Controls term frequency saturation (typically 1.2-2.0)
    pub k1: f32,
    /// Controls length normalization (typically 0.75)
    pub b: f32,
    /// Epsilon for IDF smoothing
    pub epsilon: f32,
}

impl Default for BM25Params {
    fn default() -> Self {
        Self {
            k1: 1.5,  // Slightly higher for code (more repetition is meaningful)
            b: 0.75,  // Standard length normalization
            epsilon: 0.25,
        }
    }
}

/// Document statistics for BM25
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentStats {
    pub doc_id: String,
    pub length: usize,
    pub term_frequencies: HashMap<String, u32>,
}

/// BM25 Index for fast text search
pub struct BM25Index {
    /// Document statistics
    documents: Arc<DashMap<String, DocumentStats>>,
    
    /// Inverted index: term -> list of (doc_id, term_frequency)
    inverted_index: Arc<DashMap<String, Vec<(String, u32)>>>,
    
    /// Document frequencies for each term
    doc_frequencies: Arc<DashMap<String, usize>>,
    
    /// Total number of documents
    total_docs: Arc<DashMap<String, usize>>,
    
    /// Average document length
    avg_doc_length: Arc<DashMap<String, f32>>,
    
    /// BM25 parameters
    params: BM25Params,
    
    /// Term cache for faster lookups
    term_cache: Arc<DashMap<String, Vec<String>>>,
}

impl BM25Index {
    /// Create a new BM25 index
    pub fn new(params: BM25Params) -> Self {
        Self {
            documents: Arc::new(DashMap::new()),
            inverted_index: Arc::new(DashMap::new()),
            doc_frequencies: Arc::new(DashMap::new()),
            total_docs: Arc::new(DashMap::new()),
            avg_doc_length: Arc::new(DashMap::new()),
            params,
            term_cache: Arc::new(DashMap::new()),
        }
    }
    
    /// Create with default parameters
    pub fn default() -> Self {
        Self::new(BM25Params::default())
    }
    
    /// Index a document
    pub fn index_document(&self, doc_id: String, content: &str) -> Result<()> {
        // Tokenize content
        let tokens = self.tokenize(content);
        let doc_length = tokens.len();
        
        // Calculate term frequencies
        let mut term_frequencies: HashMap<String, u32> = HashMap::new();
        for token in &tokens {
            *term_frequencies.entry(token.clone()).or_insert(0) += 1;
        }
        
        // Store document stats
        let doc_stats = DocumentStats {
            doc_id: doc_id.clone(),
            length: doc_length,
            term_frequencies: term_frequencies.clone(),
        };
        self.documents.insert(doc_id.clone(), doc_stats);
        
        // Update inverted index and document frequencies
        for (term, freq) in term_frequencies {
            // Update inverted index
            self.inverted_index
                .entry(term.clone())
                .or_insert_with(Vec::new)
                .push((doc_id.clone(), freq));
            
            // Update document frequency
            let mut df = self.doc_frequencies.entry(term).or_insert(0);
            *df += 1;
        }
        
        // Update total docs and average length
        self.update_statistics();
        
        Ok(())
    }
    
    /// Remove a document from the index
    pub fn remove_document(&self, doc_id: &str) -> Result<()> {
        // Remove from documents
        if let Some((_, doc_stats)) = self.documents.remove(doc_id) {
            // Remove from inverted index
            for (term, _) in doc_stats.term_frequencies {
                if let Some(mut posting_list) = self.inverted_index.get_mut(&term) {
                    posting_list.retain(|(id, _)| id != doc_id);
                    
                    // Update document frequency
                    if let Some(mut df) = self.doc_frequencies.get_mut(&term) {
                        *df = (*df).saturating_sub(1);
                    }
                }
            }
            
            // Update statistics
            self.update_statistics();
        }
        
        Ok(())
    }
    
    /// Search for documents using BM25 scoring
    pub fn search(&self, query: &str, limit: usize) -> Vec<(String, f32)> {
        let query_tokens = self.tokenize(query);
        
        if query_tokens.is_empty() {
            return Vec::new();
        }
        
        // Calculate IDF for query terms
        let mut query_idfs: HashMap<String, f32> = HashMap::new();
        for token in &query_tokens {
            query_idfs.insert(token.clone(), self.calculate_idf(token));
        }
        
        // Score all documents
        let mut scores: HashMap<String, f32> = HashMap::new();
        
        // Get unique query terms
        let unique_terms: HashSet<String> = query_tokens.iter().cloned().collect();
        
        for term in unique_terms {
            let idf = query_idfs.get(&term).copied().unwrap_or(0.0);
            
            // Get posting list for this term
            if let Some(posting_list) = self.inverted_index.get(&term) {
                for (doc_id, term_freq) in posting_list.iter() {
                    // Get document stats
                    if let Some(doc_stats) = self.documents.get(doc_id) {
                        let doc_length = doc_stats.length as f32;
                        let avg_length = self.get_avg_doc_length();
                        
                        // Calculate BM25 score for this term
                        let tf = *term_freq as f32;
                        let norm_factor = 1.0 - self.params.b + 
                                         self.params.b * (doc_length / avg_length);
                        let term_score = idf * (tf * (self.params.k1 + 1.0)) / 
                                        (tf + self.params.k1 * norm_factor);
                        
                        *scores.entry(doc_id.clone()).or_insert(0.0) += term_score;
                    }
                }
            }
        }
        
        // Sort by score and return top results
        let mut results: Vec<(String, f32)> = scores.into_iter().collect();
        results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);
        
        results
    }
    
    /// Advanced search with boosting for specific fields
    pub fn search_with_boost(
        &self,
        query: &str,
        field_boosts: HashMap<String, f32>,
        limit: usize,
    ) -> Vec<(String, f32)> {
        let mut base_results = self.search(query, limit * 2);
        
        // Apply field boosts
        for (doc_id, score) in &mut base_results {
            if let Some(doc_stats) = self.documents.get(doc_id) {
                // Check for special terms that indicate fields
                for (field, boost) in &field_boosts {
                    if doc_stats.term_frequencies.contains_key(field) {
                        *score *= boost;
                    }
                }
            }
        }
        
        // Re-sort after boosting
        base_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        base_results.truncate(limit);
        
        base_results
    }
    
    /// Get similar documents using term overlap
    pub fn find_similar(&self, doc_id: &str, limit: usize) -> Vec<(String, f32)> {
        if let Some(doc_stats) = self.documents.get(doc_id) {
            // Get document's terms
            let doc_terms: Vec<String> = doc_stats.term_frequencies.keys().cloned().collect();
            
            // Build a pseudo-query from document terms
            let query = doc_terms.join(" ");
            
            // Search using the document's terms
            let mut results = self.search(&query, limit + 1);
            
            // Remove the original document from results
            results.retain(|(id, _)| id != doc_id);
            results.truncate(limit);
            
            results
        } else {
            Vec::new()
        }
    }
    
    /// Tokenize text for indexing/searching
    fn tokenize(&self, text: &str) -> Vec<String> {
        // Check cache first
        if let Some(cached) = self.term_cache.get(text) {
            return cached.clone();
        }
        
        let mut tokens = Vec::new();
        
        // Split on whitespace and punctuation
        let lowercase_text = text.to_lowercase();
        let words: Vec<String> = lowercase_text
            .split(|c: char| !c.is_alphanumeric() && c != '_')
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .collect();
        
        for word in words {
            // Skip very short tokens
            if word.len() < 2 {
                continue;
            }
            
            // Add the word itself
            tokens.push(word.clone());
            
            // For programming identifiers, split on case changes
            if word.contains('_') {
                // snake_case: split on underscores
                for part in word.split('_') {
                    if part.len() >= 2 {
                        tokens.push(part.to_string());
                    }
                }
            } else {
                // camelCase/PascalCase: split on case changes
                let case_parts = self.split_camel_case(&word);
                for part in case_parts {
                    if part.len() >= 2 {
                        tokens.push(part);
                    }
                }
            }
        }
        
        // Cache the result
        if text.len() < 1000 {  // Only cache small texts
            self.term_cache.insert(text.to_string(), tokens.clone());
        }
        
        tokens
    }
    
    /// Split camelCase/PascalCase words
    fn split_camel_case(&self, word: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut current = String::new();
        
        for ch in word.chars() {
            if ch.is_uppercase() && !current.is_empty() {
                parts.push(current.clone());
                current = ch.to_lowercase().to_string();
            } else {
                current.push(ch);
            }
        }
        
        if !current.is_empty() {
            parts.push(current);
        }
        
        parts
    }
    
    /// Calculate IDF (Inverse Document Frequency) for a term
    fn calculate_idf(&self, term: &str) -> f32 {
        let total = self.get_total_docs() as f32;
        let df = self.doc_frequencies.get(term).map(|v| *v as f32).unwrap_or(0.0);
        
        if df == 0.0 {
            return 0.0;
        }
        
        ((total - df + 0.5) / (df + 0.5) + 1.0).ln()
    }
    
    /// Update index statistics
    fn update_statistics(&self) {
        let total = self.documents.len();
        self.total_docs.insert("count".to_string(), total);
        
        if total > 0 {
            let total_length: usize = self.documents.iter()
                .map(|entry| entry.value().length)
                .sum();
            let avg = total_length as f32 / total as f32;
            self.avg_doc_length.insert("avg".to_string(), avg);
        }
    }
    
    /// Get total number of documents
    fn get_total_docs(&self) -> usize {
        self.total_docs.get("count").map(|v| *v).unwrap_or(0)
    }
    
    /// Get average document length
    fn get_avg_doc_length(&self) -> f32 {
        self.avg_doc_length.get("avg").map(|v| *v).unwrap_or(1.0)
    }
    
    /// Get index statistics
    pub fn get_stats(&self) -> IndexStats {
        IndexStats {
            total_documents: self.get_total_docs(),
            total_terms: self.inverted_index.len(),
            avg_doc_length: self.get_avg_doc_length(),
            index_size_bytes: self.estimate_memory_usage(),
        }
    }
    
    /// Estimate memory usage of the index
    fn estimate_memory_usage(&self) -> usize {
        let doc_size = self.documents.len() * std::mem::size_of::<DocumentStats>();
        let index_size = self.inverted_index.iter()
            .map(|entry| {
                let term_size = entry.key().len();
                let list_size = entry.value().len() * std::mem::size_of::<(String, u32)>();
                term_size + list_size
            })
            .sum::<usize>();
        
        doc_size + index_size
    }
}

/// Index statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexStats {
    pub total_documents: usize,
    pub total_terms: usize,
    pub avg_doc_length: f32,
    pub index_size_bytes: usize,
}

/// Hybrid search result combining BM25 and vector scores
#[derive(Debug, Clone)]
pub struct HybridResult {
    pub doc_id: String,
    pub bm25_score: f32,
    pub vector_score: f32,
    pub combined_score: f32,
}

/// Combine BM25 and vector search results
pub fn hybrid_search(
    bm25_results: Vec<(String, f32)>,
    vector_results: Vec<(String, f32)>,
    alpha: f32,  // Weight for BM25 (1-alpha for vector)
) -> Vec<HybridResult> {
    let mut combined: HashMap<String, (f32, f32)> = HashMap::new();
    
    // Normalize BM25 scores
    let max_bm25 = bm25_results.iter()
        .map(|(_, score)| *score)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(1.0);
    
    for (doc_id, score) in bm25_results {
        let normalized = if max_bm25 > 0.0 { score / max_bm25 } else { 0.0 };
        combined.entry(doc_id)
            .or_insert((0.0, 0.0))
            .0 = normalized;
    }
    
    // Normalize vector scores
    let max_vector = vector_results.iter()
        .map(|(_, score)| *score)
        .max_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .unwrap_or(1.0);
    
    for (doc_id, score) in vector_results {
        let normalized = if max_vector > 0.0 { score / max_vector } else { 0.0 };
        combined.entry(doc_id)
            .or_insert((0.0, 0.0))
            .1 = normalized;
    }
    
    // Combine scores
    let mut results: Vec<HybridResult> = combined.into_iter()
        .map(|(doc_id, (bm25, vector))| {
            HybridResult {
                doc_id,
                bm25_score: bm25,
                vector_score: vector,
                combined_score: alpha * bm25 + (1.0 - alpha) * vector,
            }
        })
        .collect();
    
    // Sort by combined score
    results.sort_by(|a, b| {
        b.combined_score.partial_cmp(&a.combined_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_bm25_indexing() {
        let index = BM25Index::default();
        
        // Index some documents
        index.index_document("doc1".to_string(), "rust async function implementation").unwrap();
        index.index_document("doc2".to_string(), "python async await syntax").unwrap();
        index.index_document("doc3".to_string(), "rust struct and impl blocks").unwrap();
        
        // Search
        let results = index.search("async rust", 10);
        
        assert!(!results.is_empty());
        assert_eq!(results[0].0, "doc1");  // Should rank doc1 first
    }
    
    #[test]
    fn test_tokenization() {
        let index = BM25Index::default();
        let tokens = index.tokenize("getUserName async_function CamelCase");
        
        assert!(tokens.contains(&"getusername".to_string()));
        assert!(tokens.contains(&"get".to_string()));
        assert!(tokens.contains(&"user".to_string()));
        assert!(tokens.contains(&"name".to_string()));
        assert!(tokens.contains(&"async".to_string()));
        assert!(tokens.contains(&"function".to_string()));
    }
    
    #[test]
    fn test_hybrid_search() {
        let bm25_results = vec![
            ("doc1".to_string(), 10.0),
            ("doc2".to_string(), 8.0),
            ("doc3".to_string(), 6.0),
        ];
        
        let vector_results = vec![
            ("doc2".to_string(), 0.9),
            ("doc1".to_string(), 0.7),
            ("doc4".to_string(), 0.6),
        ];
        
        let hybrid = hybrid_search(bm25_results, vector_results, 0.5);
        
        assert_eq!(hybrid.len(), 4);  // Should have all unique documents
        assert!(hybrid[0].combined_score > hybrid[1].combined_score);
    }
}