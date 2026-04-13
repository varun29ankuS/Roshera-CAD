use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use rag_engine::search::optimized_ops::{OptimizedOps, fast_cosine, cosine_normalized_no_branch};
use std::time::Duration;

fn bench_normalization(c: &mut Criterion) {
    let mut group = c.benchmark_group("normalization");
    
    for dim in [128, 256, 512, 1024] {
        group.bench_with_input(BenchmarkId::from_parameter(dim), &dim, |b, &dim| {
            let mut ops = OptimizedOps::new();
            let mut vectors: Vec<Vec<f32>> = (0..100)
                .map(|i| vec![i as f32 / 100.0; dim])
                .collect();
            
            b.iter(|| {
                ops.batch_normalize(black_box(&mut vectors));
            });
        });
    }
    
    group.finish();
}

fn bench_dot_product(c: &mut Criterion) {
    let mut group = c.benchmark_group("dot_product");
    
    for dim in [128, 256, 512, 1024] {
        group.bench_with_input(BenchmarkId::from_parameter(dim), &dim, |b, &dim| {
            let a = vec![1.0; dim];
            let b = vec![0.5; dim];
            
            b.iter(|| {
                OptimizedOps::dot_product_dispatch(black_box(&a), black_box(&b))
            });
        });
    }
    
    group.finish();
}

fn bench_cosine_vs_dot(c: &mut Criterion) {
    let mut group = c.benchmark_group("cosine_vs_dot");
    
    // Test multiple dimensions
    for dim in [128, 256, 512, 1024] {
        let mut a = vec![1.0; dim];
        let mut b = vec![0.5; dim];
        
        // Normalize vectors
        let ops = OptimizedOps::new();
        ops.normalize_inplace(&mut a);
        ops.normalize_inplace(&mut b);
        
        group.bench_with_input(
            BenchmarkId::new("dot_product", dim),
            &dim,
            |bencher, _| {
                bencher.iter(|| {
                    OptimizedOps::dot_product_dispatch(black_box(&a), black_box(&b))
                });
            },
        );
        
        group.bench_with_input(
            BenchmarkId::new("cosine_no_branch", dim),
            &dim,
            |bencher, _| {
                bencher.iter(|| {
                    cosine_normalized_no_branch(black_box(&a), black_box(&b))
                });
            },
        );
        
        // Direct call to specific dimension function (no dispatch)
        let label = format!("dot_direct_{}", dim);
        group.bench_with_input(
            BenchmarkId::new(&label, dim),
            &dim,
            |bencher, _| {
                bencher.iter(|| {
                    match dim {
                        128 => OptimizedOps::dot_product_128(black_box(&a), black_box(&b)),
                        256 => OptimizedOps::dot_product_256(black_box(&a), black_box(&b)),
                        512 => OptimizedOps::dot_product_512(black_box(&a), black_box(&b)),
                        1024 => OptimizedOps::dot_product_1024(black_box(&a), black_box(&b)),
                        _ => OptimizedOps::dot_product_generic(black_box(&a), black_box(&b)),
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn bench_gemm(c: &mut Criterion) {
    let mut group = c.benchmark_group("gemm");
    group.measurement_time(Duration::from_secs(10));
    
    // Test 100x512 as specified
    group.bench_function("100x512x1000", |bencher| {
        let mut ops = OptimizedOps::new();
        let mut queries: Vec<Vec<f32>> = (0..100)
            .map(|i| vec![i as f32 / 100.0; 512])
            .collect();
        let database: Vec<Vec<f32>> = (0..1000)
            .map(|i| vec![i as f32 / 1000.0; 512])
            .collect();
        
        // Pre-normalize
        ops.batch_normalize(&mut queries);
        
        bencher.iter(|| {
            ops.tiled_gemm(black_box(&queries), black_box(&database))
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    bench_normalization,
    bench_dot_product,
    bench_cosine_vs_dot,
    bench_gemm
);
criterion_main!(benches);