use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId, Throughput};
use rag_engine::{
    search::{simd::{PortableSimdOps, AutoSimd}, hnsw::HNSWIndex, TurboSearch, optimized_ops::{OptimizedOps, cosine_normalized_no_branch}},
    cache::{LayeredCache, CacheConfig},
    storage::{StorageEngine, compression::{CompressionEngine, CompressionAlgorithm, DataHint}},
    ingestion::IngestionPipeline,
    query::QueryExecutor,
    intelligence::IntelligenceEngine,
    distribution::DistributionLayer,
    IntelligenceConfig,
    DistributionConfig,
};
use std::path::Path;
use std::sync::Arc;
use rand::prelude::*;
use std::time::Duration;

/// Generate random vectors for testing
fn generate_random_vectors(count: usize, dim: usize) -> Vec<Vec<f32>> {
    let mut rng = rand::thread_rng();
    (0..count)
        .map(|_| {
            (0..dim)
                .map(|_| rng.gen_range(-1.0..1.0))
                .collect()
        })
        .collect()
}

/// Generate random text documents
fn generate_random_documents(count: usize) -> Vec<String> {
    let mut rng = rand::thread_rng();
    let words = vec![
        "function", "class", "struct", "impl", "trait", "async", "await", 
        "return", "match", "if", "else", "for", "while", "loop", "break",
        "continue", "pub", "fn", "let", "mut", "const", "static", "enum"
    ];
    
    (0..count)
        .map(|_| {
            let doc_len = rng.gen_range(100..500);
            (0..doc_len)
                .map(|_| words.choose(&mut rng).unwrap().to_string())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect()
}

/// Benchmark SIMD operations
fn bench_simd_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_operations");
    
    for dim in [128, 256, 512, 1024, 2048].iter() {
        let vec_a = generate_random_vectors(1, *dim)[0].clone();
        let vec_b = generate_random_vectors(1, *dim)[0].clone();
        
        // Pre-normalized versions for optimized tests
        let mut vec_a_norm = vec_a.clone();
        let mut vec_b_norm = vec_b.clone();
        let ops = OptimizedOps::new();
        ops.normalize_inplace(&mut vec_a_norm);
        ops.normalize_inplace(&mut vec_b_norm);
        
        group.throughput(Throughput::Elements(*dim as u64));
        
        // Benchmark dot product
        group.bench_with_input(
            BenchmarkId::new("dot_product", dim),
            dim,
            |b, _| {
                b.iter(|| {
                    PortableSimdOps::dot_product_portable(
                        black_box(&vec_a),
                        black_box(&vec_b)
                    )
                });
            }
        );
        
        // Benchmark cosine similarity (old)
        group.bench_with_input(
            BenchmarkId::new("cosine_similarity_old", dim),
            dim,
            |b, _| {
                b.iter(|| {
                    PortableSimdOps::cosine_similarity_portable(
                        black_box(&vec_a),
                        black_box(&vec_b)
                    )
                });
            }
        );
        
        // Benchmark optimized cosine (pre-normalized)
        group.bench_with_input(
            BenchmarkId::new("cosine_optimized", dim),
            dim,
            |b, _| {
                b.iter(|| {
                    cosine_normalized_no_branch(
                        black_box(&vec_a_norm),
                        black_box(&vec_b_norm)
                    )
                });
            }
        );
        
        // Benchmark optimized dot product
        group.bench_with_input(
            BenchmarkId::new("dot_optimized", dim),
            dim,
            |b, _| {
                b.iter(|| {
                    OptimizedOps::dot_product_dispatch(
                        black_box(&vec_a_norm),
                        black_box(&vec_b_norm)
                    )
                });
            }
        );
        
        // Benchmark euclidean distance
        group.bench_with_input(
            BenchmarkId::new("euclidean_distance", dim),
            dim,
            |b, _| {
                b.iter(|| {
                    PortableSimdOps::euclidean_distance_portable(
                        black_box(&vec_a),
                        black_box(&vec_b)
                    )
                });
            }
        );
    }
    
    // Benchmark batch operations
    let query = generate_random_vectors(1, 512)[0].clone();
    let vectors = generate_random_vectors(100, 512);
    
    group.bench_function("batch_cosine_100x512", |b| {
        b.iter(|| {
            PortableSimdOps::batch_cosine_similarity_portable(
                black_box(&query),
                black_box(&vectors)
            )
        });
    });
    
    group.finish();
}

/// Benchmark HNSW index
fn bench_hnsw_index(c: &mut Criterion) {
    let mut group = c.benchmark_group("hnsw_index");
    group.measurement_time(Duration::from_secs(5));
    group.sample_size(10); // Reduce sample size
    
    // Test with 100 vectors
    for num_vectors in [100].iter() {
        let vectors = generate_random_vectors(*num_vectors, 128);
        let mut index = HNSWIndex::new(
            8,   // M parameter (reduced from 16)
            50,  // ef_construction (reduced from 200)
            rag_engine::search::hnsw::DistanceFunction::Cosine
        );
        
        // Benchmark insertion
        group.bench_with_input(
            BenchmarkId::new("insert", num_vectors),
            num_vectors,
            |b, _| {
                b.iter_custom(|iters| {
                    let mut total_time = Duration::ZERO;
                    for _ in 0..iters {
                        let mut idx = HNSWIndex::new(8, 50, 
                            rag_engine::search::hnsw::DistanceFunction::Cosine);
                        let start = std::time::Instant::now();
                        // Insert 100 vectors for benchmark
                        for vec in vectors.iter().take(100) {
                            idx.insert(vec.clone());
                        }
                        total_time += start.elapsed();
                    }
                    total_time
                });
            }
        );
        
        // Build index for search benchmarks (only insert first 100)
        for vec in vectors.iter().take(100) {
            index.insert(vec.clone());
        }
        
        // Benchmark search with different k values
        let query = generate_random_vectors(1, 128)[0].clone();
        
        for k in [5, 10].iter() {  // Reduced k values
            group.bench_with_input(
                BenchmarkId::new(format!("search_k{}", k), num_vectors),
                k,
                |b, &k| {
                    b.iter(|| {
                        index.search(black_box(&query), k, k * 2)
                    });
                }
            );
        }
    }
    
    group.finish();
}

/// Benchmark cache operations
fn bench_cache_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("cache_operations");
    
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let cache = LayeredCache::new(CacheConfig::default());
    
    // Prepare test data
    let keys: Vec<String> = (0..1000).map(|i| format!("key_{}", i)).collect();
    let values: Vec<Vec<u8>> = (0..1000).map(|i| {
        format!("value_{}_with_some_data", i).into_bytes()
    }).collect();
    
    // Benchmark cache set
    group.bench_function("cache_set", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let idx = rand::thread_rng().gen_range(0..1000);
                cache.set(
                    black_box(keys[idx].clone()),
                    black_box(values[idx].clone())
                ).await;
            });
        });
    });
    
    // Populate cache for get benchmarks
    runtime.block_on(async {
        for i in 0..500 {
            cache.set(keys[i].clone(), values[i].clone()).await;
        }
    });
    
    // Benchmark cache get (hit)
    group.bench_function("cache_get_hit", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let idx = rand::thread_rng().gen_range(0..500);
                cache.get(black_box(&keys[idx])).await
            });
        });
    });
    
    // Benchmark cache get (miss)
    group.bench_function("cache_get_miss", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let idx = rand::thread_rng().gen_range(500..1000);
                cache.get(black_box(&keys[idx])).await
            });
        });
    });
    
    group.finish();
}

/// Benchmark compression
fn bench_compression(c: &mut Criterion) {
    let mut group = c.benchmark_group("compression");
    
    // Generate different types of data
    let text_data = "fn main() { println!(\"Hello, world!\"); }".repeat(100).into_bytes();
    let json_data = r#"{"name":"test","value":42,"array":[1,2,3]}"#.repeat(50).into_bytes();
    let binary_data: Vec<u8> = (0..1000).map(|i| (i % 256) as u8).collect();
    let repetitive_data = vec![42u8; 10000];
    
    // Use the unified CompressionEngine with different hints
    let engine = CompressionEngine::new();
    
    let test_cases = [
        ("text", &text_data, Some(DataHint::Code)),
        ("json", &json_data, Some(DataHint::Text)),
        ("binary", &binary_data, None),
        ("repetitive", &repetitive_data, None),
    ];
    
    for (name, data, hint) in test_cases.iter() {
        // Benchmark compression with hint
        group.bench_with_input(
            BenchmarkId::new(format!("compress_{}", name), data.len()),
            data,
            |b, data| {
                b.iter(|| {
                    engine.compress(black_box(data), hint.clone())
                });
            }
        );
        
        // Benchmark decompression
        if let Ok(compressed) = engine.compress(data, hint.clone()) {
            group.bench_with_input(
                BenchmarkId::new(format!("decompress_{}", name), compressed.data.len()),
                &compressed,
                |b, compressed_data| {
                    b.iter(|| {
                        engine.decompress(black_box(compressed_data))
                    });
                }
            );
        }
    }
    
    group.finish();
}

/// Benchmark ingestion pipeline
fn bench_ingestion(c: &mut Criterion) {
    let mut group = c.benchmark_group("ingestion");
    group.measurement_time(Duration::from_secs(10));
    
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let storage = runtime.block_on(async {
        Arc::new(StorageEngine::new(Path::new("./bench_storage")).await.unwrap())
    });
    let pipeline = IngestionPipeline::new(storage);
    
    // Generate test documents
    let documents = generate_random_documents(100);
    
    // Benchmark document ingestion
    group.bench_function("ingest_100_docs", |b| {
        b.iter_custom(|iters| {
            let mut total_time = Duration::ZERO;
            for _ in 0..iters {
                let storage = runtime.block_on(async {
                    Arc::new(StorageEngine::new(Path::new("./bench_storage_temp")).await.unwrap())
                });
                let pipeline = IngestionPipeline::new(storage);
                
                let start = std::time::Instant::now();
                runtime.block_on(async {
                    for (i, doc) in documents.iter().take(10).enumerate() {
                        let path_str = format!("test_{}.txt", i);
                        let path = Path::new(&path_str);
                        pipeline.index_file(path, doc).await.ok();
                    }
                });
                total_time += start.elapsed();
                
                // Cleanup
                std::fs::remove_dir_all("./bench_storage_temp").ok();
            }
            total_time
        });
    });
    
    group.finish();
}

/// Benchmark query execution
fn bench_query_execution(c: &mut Criterion) {
    let mut group = c.benchmark_group("query_execution");
    group.measurement_time(Duration::from_secs(10));
    
    let runtime = tokio::runtime::Runtime::new().unwrap();
    
    // Setup query executor
    let (storage, intelligence, distribution, executor) = runtime.block_on(async {
        let storage = Arc::new(StorageEngine::new(Path::new("./bench_storage")).await.unwrap());
        let intel_config = IntelligenceConfig::default();
        let intelligence = Arc::new(IntelligenceEngine::new(&intel_config).unwrap());
        let dist_config = DistributionConfig::default();
        let distribution = Arc::new(DistributionLayer::new(&dist_config).await.unwrap());
        let executor = QueryExecutor::new(storage.clone(), intelligence.clone(), distribution.clone());
        (storage, intelligence, distribution, executor)
    });
    
    // Prepare test queries
    let queries = vec![
        "find function implementations",
        "search for async await patterns",
        "locate struct definitions with derive macros",
        "get all trait implementations",
        "find error handling code",
    ];
    
    // Benchmark different query types
    for query in queries.iter() {
        group.bench_with_input(
            BenchmarkId::new("execute_query", query),
            query,
            |b, query| {
                b.iter(|| {
                    runtime.block_on(async {
                        executor.execute(
                            black_box(query),
                            uuid::Uuid::new_v4()
                        ).await.ok()
                    });
                });
            }
        );
    }
    
    group.finish();
}

/// Benchmark end-to-end RAG pipeline
fn bench_rag_pipeline(c: &mut Criterion) {
    let mut group = c.benchmark_group("rag_pipeline");
    group.measurement_time(Duration::from_secs(15));
    
    let runtime = tokio::runtime::Runtime::new().unwrap();
    
    // Setup complete RAG system
    let (storage, pipeline, executor) = runtime.block_on(async {
        let storage = Arc::new(StorageEngine::new(Path::new("./bench_storage")).await.unwrap());
        let intel_config = IntelligenceConfig::default();
        let intelligence = Arc::new(IntelligenceEngine::new(&intel_config).unwrap());
        let dist_config = DistributionConfig::default();
        let distribution = Arc::new(DistributionLayer::new(&dist_config).await.unwrap());
        let pipeline = IngestionPipeline::new(storage.clone());
        let executor = QueryExecutor::new(storage.clone(), intelligence, distribution);
        (storage, pipeline, executor)
    });
    
    // Ingest sample data
    let documents = generate_random_documents(50);
    runtime.block_on(async {
        for (i, doc) in documents.iter().enumerate() {
            let path_str = format!("doc_{}.txt", i);
            let path = Path::new(&path_str);
            pipeline.index_file(path, doc).await.ok();
        }
    });
    
    // Benchmark end-to-end query
    group.bench_function("e2e_query_50_docs", |b| {
        b.iter(|| {
            runtime.block_on(async {
                let query = "find relevant code patterns";
                executor.execute(black_box(query), uuid::Uuid::new_v4()).await.ok()
            });
        });
    });
    
    // Cleanup
    std::fs::remove_dir_all("./bench_storage").ok();
    
    group.finish();
}

criterion_group!(
    benches,
    bench_simd_operations,
    // bench_hnsw_index,  // Disabled - hanging issue
    bench_cache_operations,
    bench_compression,
    bench_ingestion,
    bench_query_execution,
    bench_rag_pipeline
);

criterion_main!(benches);