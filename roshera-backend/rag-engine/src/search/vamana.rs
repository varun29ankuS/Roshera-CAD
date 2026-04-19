/// Vamana Graph Index - Core of Microsoft's DiskANN
/// 
/// Key innovations over HNSW:
/// 1. Single entry point (medoid) instead of multiple entry points
/// 2. Robust pruning with alpha parameter for high dimensions
/// 3. Exact R out-degree for every node (balanced graph)
/// 4. Better performance on high-dimensional vectors (1536 dims)
/// 
/// References:
/// - DiskANN paper: https://arxiv.org/abs/1909.11616
/// - Vamana paper: https://arxiv.org/abs/1810.07355

use dashmap::DashMap;
use std::collections::{BinaryHeap, HashSet};
use std::sync::atomic::{AtomicU32, AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use rayon::prelude::*;
use crate::search::scalar_quantization::{ScalarQuantizer, SQ1536, SQCode};

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub id: u32,
    pub distance: f32,
}

impl PartialEq for SearchResult {
    fn eq(&self, other: &Self) -> bool {
        self.distance == other.distance && self.id == other.id
    }
}

impl Eq for SearchResult {}

impl PartialOrd for SearchResult {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        // For max-heap: reverse order (larger distances first)
        other.distance.partial_cmp(&self.distance)
    }
}

impl Ord for SearchResult {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.partial_cmp(other).unwrap_or(std::cmp::Ordering::Equal)
    }
}

#[derive(Debug, Clone)]
pub enum DistanceFunction {
    Cosine,
    Euclidean,
    DotProduct,
}

/// Vamana Graph Index - Microsoft DiskANN implementation
pub struct VamanaIndex {
    /// Node vectors in Structure of Arrays format for cache efficiency
    vectors: DashMap<u32, Vec<f32>>,
    
    /// Graph adjacency lists: node_id -> Vec<neighbor_id>
    /// Each node has exactly R neighbors (except during construction)
    graph: DashMap<u32, Vec<u32>>,
    
    /// Scalar quantization for compressed search
    sq: RwLock<Option<SQ1536>>,
    sq_codes: DashMap<u32, SQCode>,
    use_sq: bool,
    sq_trained: AtomicBool,
    
    /// Graph parameters
    R: usize,           // Out-degree (number of neighbors per node)
    L: usize,           // Search list size during construction
    alpha: f32,         // Robust pruning parameter (1.2 for build, 1.0 for search)
    
    /// Single entry point (medoid of the graph)
    medoid: RwLock<Option<u32>>,
    
    /// Distance function
    distance_fn: DistanceFunction,
    
    /// Node management
    next_id: AtomicU32,
    
    /// Normalization
    normalized_vectors: bool,
    
    /// Random number generator for medoid selection
    rng: std::sync::Mutex<rand::rngs::StdRng>,
}

impl VamanaIndex {
    /// Create a new Vamana index
    /// 
    /// # Parameters
    /// - `R`: Out-degree (64 is good for 1536-dim vectors)
    /// - `L`: Search list size during construction (100-200)
    /// - `alpha`: Robust pruning parameter (1.2 for construction)
    /// - `distance_fn`: Distance function to use
    /// - `normalize`: Whether to normalize vectors
    /// - `use_sq`: Whether to use scalar quantization
    pub fn new(
        R: usize,
        L: usize,
        alpha: f32,
        distance_fn: DistanceFunction,
        normalize: bool,
        use_sq: bool,
    ) -> Self {
        let sq = if use_sq {
            Some(SQ1536::new())
        } else {
            None
        };
        
        Self {
            vectors: DashMap::new(),
            graph: DashMap::new(),
            sq: RwLock::new(sq),
            sq_codes: DashMap::new(),
            use_sq,
            sq_trained: AtomicBool::new(false),
            R,
            L,
            alpha,
            medoid: RwLock::new(None),
            distance_fn,
            next_id: AtomicU32::new(0),
            normalized_vectors: normalize,
            rng: std::sync::Mutex::new(rand::SeedableRng::seed_from_u64(42)),
        }
    }
    
    /// Insert a vector into the index
    pub fn insert(&self, vector: Vec<f32>) -> u32 {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        
        // Normalize vector if needed
        let final_vector = if self.normalized_vectors {
            Self::normalize_vector(&vector)
        } else {
            vector
        };
        
        // Store vector
        self.vectors.insert(id, final_vector.clone());
        
        // Train SQ after collecting enough vectors (e.g., 1000)
        if self.use_sq && !self.sq_trained.load(Ordering::Relaxed) && id >= 1000 {
            self.train_sq();
            self.sq_trained.store(true, Ordering::Relaxed);
            
            // Re-encode all existing vectors with trained SQ
            for entry in self.vectors.iter() {
                let vec_id = *entry.key();
                let vec_data = entry.value().clone();
                if let Some(ref sq) = *self.sq
                .read()
                .expect("vamana scalar-quantizer RwLock poisoned") {
                    let code = sq.encode(&vec_data);
                    self.sq_codes.insert(vec_id, code);
                }
            }
        }
        
        // Store SQ code if enabled and trained
        if self.use_sq && self.sq_trained.load(Ordering::Relaxed) {
            if let Some(ref sq) = *self.sq
                .read()
                .expect("vamana scalar-quantizer RwLock poisoned") {
                let code = sq.encode(&final_vector);
                self.sq_codes.insert(id, code);
            }
        }
        
        // Initialize empty neighbor list
        self.graph.insert(id, Vec::new());
        
        // If this is the first node, make it the medoid
        if id == 0 {
            *self.medoid
            .write()
            .expect("vamana medoid RwLock poisoned") = Some(id);
            return id;
        }
        
        // Find neighbors using greedy search from medoid
        // Medoid was initialized at id==0 (early return above) before any
        // subsequent insert can reach this branch, so `Some` is an invariant.
        let medoid_id = self
            .medoid
            .read()
            .expect("vamana medoid RwLock poisoned")
            .expect("vamana medoid must be Some once id > 0 (first insert sets it)");
        let candidates = self.greedy_search(&final_vector, medoid_id, self.L);
        
        // Robust pruning to select exactly R neighbors
        let neighbors = self.robust_prune(&final_vector, candidates, self.R, self.alpha);
        
        // Add bidirectional edges
        for &neighbor_id in &neighbors {
            // Add neighbor to new node
            self.graph
                .get_mut(&id)
                .expect("vamana: new node's graph entry was just inserted above").push(neighbor_id);
            
            // Add new node to neighbor
            self.graph
                .get_mut(&neighbor_id)
                .expect("vamana: neighbor_id originates from the graph itself").push(id);
            
            // Prune neighbor if it exceeds R connections
            let neighbor_connections = self
                .graph
                .get(&neighbor_id)
                .expect("vamana: neighbor_id originates from the graph itself")
                .clone();
            if neighbor_connections.len() > self.R {
                let neighbor_vector = self
                .vectors
                .get(&neighbor_id)
                .expect("vamana: neighbor_id originates from graph; vectors entry must exist")
                .clone();
                
                // Convert neighbor connections to SearchResults with distances
                let neighbor_candidates: Vec<SearchResult> = neighbor_connections.iter().map(|&nid| {
                    let dist = if nid == neighbor_id {
                        0.0
                    } else {
                        let other_vector = self.vectors.get(&nid).expect(
                            "vamana: nid originates from graph; vectors entry must exist",
                        );
                        self.calculate_distance(&neighbor_vector, &other_vector)
                    };
                    SearchResult { id: nid, distance: dist }
                }).collect();
                
                let pruned = self.robust_prune(&neighbor_vector, neighbor_candidates, self.R, self.alpha);
                
                // Update neighbor's connections
                *self.graph
                .get_mut(&neighbor_id)
                .expect("vamana: neighbor_id originates from the graph itself") = pruned;
            }
        }
        
        // Periodically update medoid (every 1000 insertions)
        if id % 1000 == 0 && id > 0 {
            self.update_medoid();
        }
        
        id
    }
    
    /// Search for k nearest neighbors
    pub fn search(&self, query: &[f32], k: usize, L: Option<usize>) -> Vec<SearchResult> {
        let search_L = L.unwrap_or(std::cmp::max(k * 2, 50));
        
        // Normalize query if needed
        let query_vector = if self.normalized_vectors {
            Self::normalize_vector(query)
        } else {
            query.to_vec()
        };
        
        // Start search from medoid
        let medoid_guard = self
            .medoid
            .read()
            .expect("vamana medoid RwLock poisoned");
        let medoid_id = match medoid_guard.as_ref() {
            Some(&id) => id,
            None => return Vec::new(),
        };
        
        // Use compressed search if SQ is enabled
        let mut candidates = if self.use_sq {
            self.compressed_search(&query_vector, medoid_id, search_L)
        } else {
            self.greedy_search(&query_vector, medoid_id, search_L)
        };
        
        // Rerank with exact distances if using SQ
        if self.use_sq {
            for result in &mut candidates {
                let exact_vector = self
                    .vectors
                    .get(&result.id)
                    .expect("vamana rerank: result.id originates from graph search; vectors entry must exist");
                result.distance = self.calculate_distance(&query_vector, &exact_vector);
            }
            candidates.sort_by(|a, b| a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal));
        }
        
        // Return top k
        candidates.truncate(k);
        candidates
    }
    
    /// Greedy search from a starting point
    fn greedy_search(&self, query: &[f32], start: u32, L: usize) -> Vec<SearchResult> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new(); // Max heap for furthest
        let mut w = BinaryHeap::new(); // Max heap for result set
        
        // Start with the starting point
        let start_vec = self
            .vectors
            .get(&start)
            .expect("vamana greedy_search: start id must have a vectors entry");
        let start_dist = self.calculate_distance(query, &start_vec);
        candidates.push(SearchResult { id: start, distance: -start_dist }); // Negative for min-heap behavior
        w.push(SearchResult { id: start, distance: start_dist });
        visited.insert(start);
        
        while let Some(current) = candidates.pop() {
            let current_id = current.id;
            let current_dist = -current.distance; // Convert back to positive
            
            // If current is further than the furthest in w, stop
            if let Some(furthest) = w.peek() {
                if current_dist > furthest.distance {
                    break;
                }
            }
            
            // Explore neighbors
            if let Some(neighbors) = self.graph.get(&current_id) {
                for &neighbor_id in neighbors.iter() {
                    if !visited.contains(&neighbor_id) {
                        visited.insert(neighbor_id);
                        
                        let neighbor_vec = self.vectors.get(&neighbor_id).expect(
                            "vamana: neighbor_id originates from graph; vectors entry must exist",
                        );
                        let neighbor_dist = self.calculate_distance(query, &neighbor_vec);
                        
                        // Add to candidates for further exploration
                        candidates.push(SearchResult { id: neighbor_id, distance: -neighbor_dist });
                        
                        // Add to result set
                        w.push(SearchResult { id: neighbor_id, distance: neighbor_dist });
                        
                        // Keep only best L candidates
                        if w.len() > L {
                            w.pop(); // Remove furthest
                        }
                    }
                }
            }
        }
        
        // Convert heap to sorted vector
        let mut results: Vec<SearchResult> = w.into_sorted_vec();
        results.reverse(); // Sort by distance ascending
        results
    }
    
    /// Compressed search using Scalar Quantization for speed
    fn compressed_search(&self, query: &[f32], start: u32, L: usize) -> Vec<SearchResult> {
        let mut visited = HashSet::new();
        let mut candidates = BinaryHeap::new();
        let mut w = BinaryHeap::new();
        
        // Start with starting point using SQ distance
        let start_dist = self.calculate_distance_sq(query, start);
        candidates.push(SearchResult { id: start, distance: -start_dist });
        w.push(SearchResult { id: start, distance: start_dist });
        visited.insert(start);
        
        while let Some(current) = candidates.pop() {
            let current_id = current.id;
            let current_dist = -current.distance;
            
            if let Some(furthest) = w.peek() {
                if current_dist > furthest.distance {
                    break;
                }
            }
            
            // Explore neighbors using SQ distances for speed
            if let Some(neighbors) = self.graph.get(&current_id) {
                for &neighbor_id in neighbors.iter() {
                    if !visited.contains(&neighbor_id) {
                        visited.insert(neighbor_id);
                        
                        let neighbor_dist = self.calculate_distance_sq(query, neighbor_id);
                        
                        candidates.push(SearchResult { id: neighbor_id, distance: -neighbor_dist });
                        w.push(SearchResult { id: neighbor_id, distance: neighbor_dist });
                        
                        if w.len() > L {
                            w.pop();
                        }
                    }
                }
            }
        }
        
        let mut results: Vec<SearchResult> = w.into_sorted_vec();
        results.reverse();
        results
    }
    
    /// Robust pruning algorithm - key innovation of Vamana
    /// 
    /// Unlike HNSW's simple "take M closest", this ensures graph connectivity
    /// by preferring nodes that are not "covered" by already selected nodes
    fn robust_prune(&self, _query: &[f32], candidates: Vec<SearchResult>, R: usize, alpha: f32) -> Vec<u32> {
        if candidates.is_empty() {
            return Vec::new();
        }
        
        let mut result = Vec::new();
        let mut candidates = candidates;
        
        // Sort candidates by distance to query
        candidates.sort_by(|a, b| a.distance
                .partial_cmp(&b.distance)
                .unwrap_or(std::cmp::Ordering::Equal));
        
        for candidate in &candidates {
            if result.len() >= R {
                break;
            }
            
            // Check if candidate is "covered" by any node already in result
            let mut is_covered = false;
            
            for &selected_id in &result {
                // Calculate distance between candidate and selected node
                let dist_to_selected = self.calculate_distance_between(candidate.id, selected_id);
                
                // If distance to selected node < alpha * distance to query, 
                // then candidate is "covered" by selected node
                if dist_to_selected < alpha * candidate.distance {
                    is_covered = true;
                    break;
                }
            }
            
            // Add candidate if it's not covered (ensures diversity)
            if !is_covered {
                result.push(candidate.id);
            }
        }
        
        // If we don't have enough neighbors, add more from candidates
        // This can happen when alpha is too strict
        if result.len() < R {
            for candidate in &candidates {
                if result.len() >= R {
                    break;
                }
                if !result.contains(&candidate.id) {
                    result.push(candidate.id);
                }
            }
        }
        
        result
    }
    
    /// Calculate distance between two nodes by ID
    fn calculate_distance_between(&self, id1: u32, id2: u32) -> f32 {
        if id1 == id2 {
            return 0.0;
        }
        
        // Caller provides ids from the graph / vectors maps.
        let vec1 = self
            .vectors
            .get(&id1)
            .expect("vamana calculate_distance_between: id1 must exist in vectors")
            .clone();
        let vec2 = self
            .vectors
            .get(&id2)
            .expect("vamana calculate_distance_between: id2 must exist in vectors")
            .clone();
        self.calculate_distance(&vec1, &vec2)
    }
    
    /// Calculate distance using SQ codes (faster)
    fn calculate_distance_sq(&self, query: &[f32], node_id: u32) -> f32 {
        if let Some(code) = self.sq_codes.get(&node_id) {
            if let Some(ref sq) = *self.sq
                .read()
                .expect("vamana scalar-quantizer RwLock poisoned") {
                return sq.distance(query, &*code);
            }
        }
        
        // Fallback to exact distance
        let node_vector = self
            .vectors
            .get(&node_id)
            .expect("vamana calculate_distance_sq: node_id must exist in vectors");
        self.calculate_distance(query, &node_vector)
    }
    
    /// Calculate exact distance between vectors
    fn calculate_distance(&self, a: &[f32], b: &[f32]) -> f32 {
        match self.distance_fn {
            DistanceFunction::Cosine => {
                let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
                1.0 - dot // Assumes normalized vectors
            }
            DistanceFunction::Euclidean => {
                a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum::<f32>().sqrt()
            }
            DistanceFunction::DotProduct => {
                -a.iter().zip(b.iter()).map(|(x, y)| x * y).sum::<f32>() // Negative for similarity->distance
            }
        }
    }
    
    /// Normalize a vector to unit length
    fn normalize_vector(vector: &[f32]) -> Vec<f32> {
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            vector.to_vec()
        } else {
            vector.iter().map(|x| x / norm).collect()
        }
    }
    
    /// Train the scalar quantizer on collected vectors
    fn train_sq(&self) {
        if !self.use_sq {
            return;
        }
        
        // Collect sample vectors for training (up to 10K samples)
        let sample_size = std::cmp::min(10000, self.vectors.len());
        let mut training_vectors = Vec::with_capacity(sample_size);
        
        for (i, entry) in self.vectors.iter().enumerate() {
            if i >= sample_size {
                break;
            }
            training_vectors.push(entry.value().clone());
        }
        
        // Train the quantizer
        let mut sq_guard = self
            .sq
            .write()
            .expect("vamana scalar-quantizer RwLock poisoned");
        if let Some(ref mut sq) = *sq_guard {
            sq.train(&training_vectors);
        }
    }
    
    /// Update medoid to the node with minimum average distance to all other nodes
    /// This is expensive but crucial for search quality
    fn update_medoid(&self) {
        let node_ids: Vec<u32> = self.vectors.iter().map(|entry| *entry.key()).collect();
        
        if node_ids.len() < 10 {
            return; // Not enough nodes to update medoid
        }
        
        // Sample random subset for efficiency (1000 nodes max)
        let sample_size = std::cmp::min(1000, node_ids.len());
        let mut rng = self
            .rng
            .lock()
            .expect("vamana RNG Mutex poisoned");
        let mut sampled_nodes = node_ids.clone();
        
        // Shuffle and take first sample_size nodes
        use rand::seq::SliceRandom;
        sampled_nodes.shuffle(&mut *rng);
        sampled_nodes.truncate(sample_size);
        
        // Find node with minimum average distance to sample
        let mut best_medoid = sampled_nodes[0];
        let mut best_avg_dist = f32::MAX;
        
        for &candidate_id in &sampled_nodes {
            let candidate_vector = self
                .vectors
                .get(&candidate_id)
                .expect("vamana update_medoid: candidate_id sampled from vectors itself");
            let total_dist: f32 = sampled_nodes.iter()
                .map(|&other_id| {
                    if other_id == candidate_id {
                        0.0
                    } else {
                        let other_vector = self.vectors.get(&other_id).expect(
                            "vamana update_medoid: other_id sampled from vectors itself",
                        );
                        self.calculate_distance(&candidate_vector, &other_vector)
                    }
                })
                .sum();
            
            let avg_dist = total_dist / sampled_nodes.len() as f32;
            if avg_dist < best_avg_dist {
                best_avg_dist = avg_dist;
                best_medoid = candidate_id;
            }
        }
        
        *self.medoid
            .write()
            .expect("vamana medoid RwLock poisoned") = Some(best_medoid);
    }
    
    /// Get statistics about the index
    pub fn stats(&self) -> VamanaStats {
        let num_nodes = self.vectors.len();
        let total_edges: usize = self.graph.iter().map(|entry| entry.value().len()).sum();
        let avg_degree = if num_nodes > 0 { total_edges as f32 / num_nodes as f32 } else { 0.0 };
        
        VamanaStats {
            num_nodes,
            total_edges,
            avg_degree,
            target_degree: self.R,
            medoid: *self
                .medoid
                .read()
                .expect("vamana medoid RwLock poisoned"),
            using_sq: self.use_sq,
        }
    }
}

#[derive(Debug)]
pub struct VamanaStats {
    pub num_nodes: usize,
    pub total_edges: usize,
    pub avg_degree: f32,
    pub target_degree: usize,
    pub medoid: Option<u32>,
    pub using_sq: bool,
}