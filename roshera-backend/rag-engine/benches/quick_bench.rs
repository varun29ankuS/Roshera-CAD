use criterion::{black_box, criterion_group, criterion_main, Criterion};
use rag_engine::search::simd::PortableSimdOps;
use rand::prelude::*;

fn bench_simd_basic(c: &mut Criterion) {
    let mut group = c.benchmark_group("simd_basic");
    
    // Test vectors of different sizes
    let sizes = vec![128, 256, 512, 1024];
    
    for size in sizes {
        let mut rng = rand::thread_rng();
        let vec_a: Vec<f32> = (0..size).map(|_| rng.gen_range(-1.0..1.0)).collect();
        let vec_b: Vec<f32> = (0..size).map(|_| rng.gen_range(-1.0..1.0)).collect();
        
        group.bench_function(format!("dot_product_{}", size), |b| {
            b.iter(|| {
                PortableSimdOps::dot_product_portable(
                    black_box(&vec_a),
                    black_box(&vec_b)
                )
            });
        });
        
        group.bench_function(format!("cosine_similarity_{}", size), |b| {
            b.iter(|| {
                PortableSimdOps::cosine_similarity_portable(
                    black_box(&vec_a),
                    black_box(&vec_b)
                )
            });
        });
    }
    
    // Benchmark batch operations
    let query: Vec<f32> = (0..512).map(|_| rand::random::<f32>()).collect();
    let vectors: Vec<Vec<f32>> = (0..100)
        .map(|_| (0..512).map(|_| rand::random::<f32>()).collect())
        .collect();
    
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

criterion_group!(benches, bench_simd_basic);
criterion_main!(benches);