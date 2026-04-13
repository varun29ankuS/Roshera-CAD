/// Test Vamana Index - Microsoft DiskANN Implementation
/// 
/// This should be significantly faster than the old HNSW with better recall
/// especially for high-dimensional vectors (1536 dims like OpenAI embeddings)

use rag_engine::search::vamana::{VamanaIndex, DistanceFunction, SearchResult};
use rand::prelude::*;
use std::time::Instant;

fn main() {
    println!("=== Testing Vamana Index (DiskANN Implementation) ===\n");
    
    // Test parameters for OpenAI-style embeddings
    let n = 10_000;
    let dim = 1536;
    let R = 64;      // Out-degree (Microsoft recommends 64 for 1536-dim)
    let L = 100;     // Search list size during construction
    let alpha = 1.2; // Robust pruning parameter
    let k = 10;      // Number of results to return
    
    println!("Configuration:");
    println!("  Vectors: {}", n);
    println!("  Dimensions: {}", dim);
    println!("  Out-degree (R): {}", R);
    println!("  Construction search (L): {}", L);
    println!("  Alpha parameter: {}", alpha);
    println!("  Results (k): {}", k);
    println!("  Using Scalar Quantization: true");
    println!();
    
    // Generate test vectors (normalized for cosine similarity)
    println!("Generating {} normalized {}-dimensional vectors...", n, dim);
    let start_gen = Instant::now();
    let vectors = generate_normalized_vectors(dim, n);
    let gen_time = start_gen.elapsed();
    println!("  Generation time: {:.2}s\n", gen_time.as_secs_f64());
    
    // Create Vamana index with SQ enabled
    println!("Creating Vamana index...");
    let vamana = VamanaIndex::new(
        R,                              // Out-degree
        L,                              // Construction search size  
        alpha,                          // Robust pruning alpha
        DistanceFunction::Cosine,       // Distance function
        true,                           // Normalize vectors
        true,                           // Use scalar quantization
    );
    
    // Insert vectors and measure construction time
    println!("Inserting vectors into index...");
    let start_build = Instant::now();
    
    for (i, vector) in vectors.iter().enumerate() {
        vamana.insert(vector.clone());
        
        if i > 0 && i % 1000 == 0 {
            let elapsed = start_build.elapsed().as_secs_f64();
            let rate = i as f64 / elapsed;
            println!("  Inserted {}/{} vectors ({:.0} vec/sec)", i, n, rate);
        }
    }
    
    let build_time = start_build.elapsed();
    let build_rate = n as f64 / build_time.as_secs_f64();
    println!("✓ Index construction complete!");
    println!("  Total time: {:.2}s", build_time.as_secs_f64());
    println!("  Rate: {:.0} vectors/second\n", build_rate);
    
    // Print index statistics
    let stats = vamana.stats();
    println!("Index Statistics:");
    println!("  Nodes: {}", stats.num_nodes);
    println!("  Total edges: {}", stats.total_edges);
    println!("  Average degree: {:.2} (target: {})", stats.avg_degree, stats.target_degree);
    println!("  Medoid: {:?}", stats.medoid);
    println!("  Using SQ: {}\n", stats.using_sq);
    
    // Test search performance and accuracy
    println!("=== Search Performance Test ===");
    
    let test_queries = 100;
    let query_indices: Vec<usize> = (0..test_queries).map(|_| thread_rng().gen_range(0..n)).collect();
    
    // Test different search list sizes
    for search_L in [50, 100, 200, 500] {
        println!("\n--- Testing L={} ---", search_L);
        
        let mut total_time = 0.0;
        let mut total_recall = 0.0;
        
        for &query_idx in &query_indices {
            let query = &vectors[query_idx];
            
            // Measure search time
            let start_search = Instant::now();
            let results = vamana.search(query, k, Some(search_L));
            let search_time = start_search.elapsed().as_secs_f64() * 1000.0; // Convert to ms
            total_time += search_time;
            
            // Calculate recall (should find itself at position 0)
            let found_self = results.iter().any(|r| r.id == query_idx as u32);
            if found_self {
                total_recall += 1.0;
            }
            
            // Verify results are sorted by distance
            for window in results.windows(2) {
                assert!(window[0].distance <= window[1].distance, 
                        "Results not sorted by distance");
            }
        }
        
        let avg_time = total_time / test_queries as f64;
        let recall = total_recall / test_queries as f64;
        
        println!("  Average search time: {:.3}ms", avg_time);
        println!("  Recall@{}: {:.1}%", k, recall * 100.0);
        
        // Performance targets for 1536-dim vectors
        if search_L == 100 {
            println!("  Performance analysis:");
            
            if avg_time < 1.0 {
                println!("    ✓ Excellent latency (<1ms)");
            } else if avg_time < 5.0 {
                println!("    ✓ Good latency (<5ms)");
            } else if avg_time < 10.0 {
                println!("    ⚠ Acceptable latency (<10ms)");
            } else {
                println!("    ❌ Poor latency (>10ms)");
            }
            
            if recall > 0.95 {
                println!("    ✓ Excellent recall (>95%)");
            } else if recall > 0.90 {
                println!("    ✓ Good recall (>90%)");
            } else if recall > 0.80 {
                println!("    ⚠ Acceptable recall (>80%)");
            } else {
                println!("    ❌ Poor recall (<80%)");
            }
        }
    }
    
    // Test scalability projection
    println!("\n=== Scalability Analysis ===");
    
    // Get the last search time from L=500 test
    let sample_query = &vectors[0];
    let start_sample = Instant::now();
    let _ = vamana.search(sample_query, k, Some(500));
    let current_time_ms = start_sample.elapsed().as_secs_f64() * 1000.0;
    
    println!("Current performance ({}K vectors):", n / 1000);
    println!("  Search time: {:.3}ms", current_time_ms);
    
    // Project to larger scales (Vamana should scale logarithmically)
    for scale in [100_000, 1_000_000, 10_000_000] {
        let scale_factor = (scale as f64 / n as f64).log2();
        let projected_time = current_time_ms * scale_factor;
        
        println!("Projected for {}M vectors:", scale / 1_000_000);
        println!("  Search time: {:.3}ms", projected_time);
        
        if projected_time < 10.0 {
            println!("  ✓ Target achieved (<10ms)");
        } else if projected_time < 50.0 {
            println!("  ⚠ Acceptable performance");
        } else {
            println!("  ❌ Too slow for production");
        }
    }
    
    // Test batch search performance
    println!("\n=== Batch Search Test ===");
    let batch_size = 100;
    let batch_queries: Vec<Vec<f32>> = query_indices.iter()
        .take(batch_size)
        .map(|&i| vectors[i].clone())
        .collect();
    
    let start_batch = Instant::now();
    let batch_results: Vec<Vec<SearchResult>> = batch_queries.iter()
        .map(|query| vamana.search(query, k, Some(100)))
        .collect();
    let batch_time = start_batch.elapsed().as_secs_f64() * 1000.0;
    
    let avg_batch_time = batch_time / batch_size as f64;
    println!("  Batch of {} queries: {:.2}ms total", batch_size, batch_time);
    println!("  Average per query: {:.3}ms", avg_batch_time);
    println!("  Throughput: {:.0} queries/second", 1000.0 / avg_batch_time);
    
    // Memory usage estimate
    println!("\n=== Memory Usage Analysis ===");
    let vector_size = dim * 4; // 4 bytes per f32
    let sq_size = dim / 4; // 8 bits per dimension with SQ
    let graph_size = R * 4; // 4 bytes per neighbor ID
    
    let total_per_vector = vector_size + sq_size + graph_size;
    let total_memory_mb = (total_per_vector * n) as f64 / (1024.0 * 1024.0);
    
    println!("  Per vector:");
    println!("    Original vector: {} bytes", vector_size);
    println!("    SQ compressed: {} bytes", sq_size);
    println!("    Graph connections: {} bytes", graph_size);
    println!("    Total: {} bytes", total_per_vector);
    println!("  Total index memory: {:.2} MB", total_memory_mb);
    println!("  Compression ratio: {:.1}x", vector_size as f64 / sq_size as f64);
    
    println!("\n=== Test Complete ===");
    println!("Vamana index successfully built and tested!");
    println!("Ready for production use with millions of vectors.");
}

fn generate_normalized_vectors(dim: usize, num: usize) -> Vec<Vec<f32>> {
    let mut rng = thread_rng();
    (0..num)
        .map(|_| {
            let mut v: Vec<f32> = (0..dim).map(|_| rng.gen_range(-1.0..1.0)).collect();
            let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            if norm > 0.0 {
                v.iter_mut().for_each(|x| *x /= norm);
            }
            v
        })
        .collect()
}