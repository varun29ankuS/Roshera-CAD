use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use rag_engine::search::hnsw::{HNSWIndex, DistanceFunction};
use rand::prelude::*;
use std::collections::HashSet;
use std::time::Duration;

/// Generate random vectors
fn generate_vectors(count: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = thread_rng();
    (0..count)
        .map(|i| {
            // Generate random values from normal distribution for better spread
            let vec: Vec<f32> = (0..dim).map(|_| {
                // Use range -1 to 1 for better distribution
                rng.gen_range(-1.0..1.0)
            }).collect();
            
            // DON'T normalize here - HNSW will normalize internally for cosine similarity
            // This avoids double normalization which causes floating point errors
            vec
        })
        .collect()
}

/// Compute exact k-NN for ground truth
fn exact_knn(query: &[f32], database: &[Vec<f32>], k: usize) -> Vec<usize> {
    let mut scores: Vec<(usize, f32)> = database
        .iter()
        .enumerate()
        .map(|(idx, vec)| {
            // Compute cosine distance properly for non-normalized vectors
            // cosine_distance = 1 - cosine_similarity
            // cosine_similarity = dot(a,b) / (norm(a) * norm(b))
            let dot: f32 = query.iter().zip(vec).map(|(a, b)| a * b).sum();
            let norm_q: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
            let norm_v: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
            
            let distance = if norm_q == 0.0 || norm_v == 0.0 {
                2.0  // Maximum distance for zero vectors
            } else {
                let similarity = dot / (norm_q * norm_v);
                1.0 - similarity
            };
            (idx, distance)
        })
        .collect();
    
    // Sort by distance ascending (lowest distance = most similar)
    scores.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    scores.into_iter().take(k).map(|(idx, _)| idx).collect()
}

/// Calculate recall@k
fn calculate_recall(exact: &[usize], approx: &[usize]) -> f32 {
    let exact_set: HashSet<_> = exact.iter().collect();
    let approx_set: HashSet<_> = approx.iter().collect();
    let intersection = exact_set.intersection(&approx_set).count();
    intersection as f32 / exact.len() as f32
}

/// Benchmark ANN with recall metrics
fn bench_ann_recall(c: &mut Criterion) {
    let mut group = c.benchmark_group("ann_recall");
    group.measurement_time(Duration::from_secs(30));
    group.sample_size(10);
    
    // Test parameters
    let database_size = 10000;
    let dim = 512;
    let num_queries = 100;
    let k_values = vec![1, 10, 50, 100];
    
    // Generate data
    println!("Generating {} vectors of dimension {}...", database_size, dim);
    let database = generate_vectors(database_size, dim);
    let queries = generate_vectors(num_queries, dim);
    
    // Different HNSW configurations
    let configs = vec![
        ("M16_ef200", 16, 200, 200),  // Back to original parameters
        // ("M8_ef50", 8, 50, 50),     // Fast but lower recall
        // ("M16_ef100", 16, 100, 100), // Balanced
        // ("M32_ef200", 32, 200, 200), // High recall
        // ("M16_ef500", 16, 100, 500), // High search effort
    ];
    
    for (name, m, ef_construction, ef_search) in configs {
        println!("\nBuilding HNSW index: {}", name);
        
        // Build index - IMPORTANT: HNSW normalizes vectors internally for cosine
        let mut index = HNSWIndex::new(m, ef_construction, DistanceFunction::Cosine);
        
        // println!("Starting to insert {} vectors into HNSW index", database.len());
        let start_time = std::time::Instant::now();
        
        for (i, vec) in database.iter().enumerate() {
            if i > 0 && i % 1000 == 0 {  // Only print after actual insertions
                let elapsed = start_time.elapsed();
                println!("Inserted {}/{} vectors in {:?}", i, database.len(), elapsed);
                
                // Timeout after 60 seconds per 1000 vectors
                if elapsed.as_secs() > 60 * ((i / 1000) + 1) as u64 {
                    println!("WARNING: Insertion taking too long, skipping rest of index build");
                    break;
                }
            }
            
            // println!("Benchmark: About to insert vector {}", i);
            index.insert(vec.clone());
            // println!("Benchmark: Inserted vector {} as node {}", i, node_id);
        }
        
        let total_time = start_time.elapsed();
        println!("Finished inserting {} vectors in {:?}", database.len(), total_time);
        
        // Debug: Check index stats
        let stats = index.stats();
        println!("Index stats: {} nodes, avg connections: {:.1}, level distribution: {:?}", 
                 stats.node_count, stats.avg_connections, 
                 &stats.level_distribution[..5.min(stats.level_distribution.len())]);
        
        // Check connectivity
        let (reachable, total) = index.check_connectivity();
        println!("Connectivity check: {}/{} nodes reachable from entry point", reachable, total);
        
        // Debug: check what the entry point is
        println!("Entry point node ID: {:?}", index.get_entry_point());
        
        // Repair disconnected components if needed
        if reachable < total {
            println!("Repairing connectivity...");
            let repaired = index.repair_connectivity();
            println!("Repaired {} connections", repaired);
            
            // Check again
            let (new_reachable, new_total) = index.check_connectivity();
            println!("After repair: {}/{} nodes reachable", new_reachable, new_total);
        }
        
        // Test different k values
        for &k in &k_values {
            // Calculate recall
            let mut total_recall = 0.0;
            let mut total_time = Duration::ZERO;
            
            for (q_idx, query) in queries.iter().enumerate() {
                // Exact search
                let exact_results = exact_knn(query, &database, k);
                
                // Approximate search
                let start = std::time::Instant::now();
                let ann_results = index.search(query, k, ef_search);
                total_time += start.elapsed();
                
                let ann_indices: Vec<usize> = ann_results
                    .iter()
                    .map(|r| r.id as usize)
                    .collect();
                
                // Debug first query to see what's happening
                if q_idx == 0 && k == 10 {
                    println!("Debug query 0, k=10:");
                    println!("  Exact results (first 5): {:?}", &exact_results[..5.min(exact_results.len())]);
                    println!("  HNSW results (first 5): {:?}", &ann_indices[..5.min(ann_indices.len())]);
                    
                    // Check distances
                    if !ann_results.is_empty() {
                        println!("  HNSW distances (first 5): {:?}", 
                            ann_results.iter().take(5).map(|r| r.distance).collect::<Vec<_>>());
                    }
                    
                    // Compute exact distances for comparison
                    let exact_distances: Vec<f32> = exact_results.iter().take(5)
                        .map(|&idx| {
                            let dot: f32 = query.iter().zip(&database[idx]).map(|(a, b)| a * b).sum();
                            let norm_q: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
                            let norm_v: f32 = database[idx].iter().map(|x| x * x).sum::<f32>().sqrt();
                            if norm_q == 0.0 || norm_v == 0.0 {
                                2.0
                            } else {
                                1.0 - (dot / (norm_q * norm_v))
                            }
                        })
                        .collect();
                    println!("  Exact distances (first 5): {:?}", exact_distances);
                    
                    // Check original vector norms (before HNSW normalization)
                    let query_norm: f32 = query.iter().map(|x| x * x).sum::<f32>().sqrt();
                    let db_norm: f32 = database[exact_results[0]].iter().map(|x| x * x).sum::<f32>().sqrt();
                    println!("  Original query norm: {}, Original DB[{}] norm: {}", query_norm, exact_results[0], db_norm);
                }
                
                total_recall += calculate_recall(&exact_results, &ann_indices);
            }
            
            let avg_recall = total_recall / queries.len() as f32;
            let avg_latency = total_time.as_micros() as f64 / queries.len() as f64;
            
            println!("  k={:3}: recall={:.3}, latency={:.1}µs", k, avg_recall, avg_latency);
            
            // Benchmark search
            group.bench_with_input(
                BenchmarkId::new(format!("{}_k{}", name, k), k),
                &k,
                |b, &k| {
                    let query = &queries[0];
                    b.iter(|| {
                        index.search(black_box(query), k, ef_search)
                    });
                },
            );
        }
    }
    
    group.finish();
}

/// Benchmark index build time
fn bench_index_build(c: &mut Criterion) {
    let mut group = c.benchmark_group("index_build");
    group.measurement_time(Duration::from_secs(60));
    group.sample_size(10);
    
    for size in [1000, 5000, 10000, 50000] {
        let vectors = generate_vectors(size, 512);
        
        group.bench_with_input(
            BenchmarkId::new("hnsw_build", size),
            &size,
            |b, _| {
                b.iter_custom(|iters| {
                    let mut total = Duration::ZERO;
                    for _ in 0..iters {
                        let mut index = HNSWIndex::new(16, 100, DistanceFunction::Cosine);
                        let start = std::time::Instant::now();
                        for vec in &vectors {
                            index.insert(vec.clone());
                        }
                        total += start.elapsed();
                    }
                    total
                });
            },
        );
    }
    
    group.finish();
}

/// Quality vs Performance tradeoff
fn bench_quality_tradeoff(c: &mut Criterion) {
    let mut group = c.benchmark_group("quality_tradeoff");
    
    let database = generate_vectors(10000, 512);
    let queries = generate_vectors(100, 512);
    let k = 10;
    
    // Build index once
    let mut index = HNSWIndex::new(16, 100, DistanceFunction::Cosine);
    for vec in &database {
        index.insert(vec.clone());
    }
    
    // Test different ef values
    for ef in [10, 20, 50, 100, 200, 500] {
        // Measure recall
        let mut total_recall = 0.0;
        for query in &queries {
            let exact = exact_knn(query, &database, k);
            let approx = index.search(query, k, ef);
            let approx_ids: Vec<usize> = approx.iter().map(|r| r.id as usize).collect();
            total_recall += calculate_recall(&exact, &approx_ids);
        }
        let avg_recall = total_recall / queries.len() as f32;
        
        println!("ef={}: recall@{}={:.3}", ef, k, avg_recall);
        
        // Benchmark latency
        group.bench_with_input(
            BenchmarkId::new("search_ef", ef),
            &ef,
            |b, &ef| {
                let query = &queries[0];
                b.iter(|| {
                    index.search(black_box(query), k, ef)
                });
            },
        );
    }
    
    group.finish();
}

criterion_group!(
    benches,
    bench_ann_recall,
    bench_index_build,
    bench_quality_tradeoff
);
criterion_main!(benches);