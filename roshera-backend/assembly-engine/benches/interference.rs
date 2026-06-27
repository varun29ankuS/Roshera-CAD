//! Benchmark: static interference sweep across a representative assembly.
//!
//! BENCHMARK↔VERIFY (loop S3). The Phase-1 sweep is O(n²) (broad-phase BVH is a
//! later slice); this pins the cost on a 50-part assembly so a regression — or
//! the broad-phase win when it lands — is visible.

use assembly_engine::{Assembly, Instance, InstanceId, Mesh};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// An axis-aligned cube of side `2*h` centred at the origin (local frame).
fn cube(h: f64) -> Mesh {
    Mesh {
        vertices: vec![
            [-h, -h, -h],
            [h, -h, -h],
            [h, h, -h],
            [-h, h, -h],
            [-h, -h, h],
            [h, -h, h],
            [h, h, h],
            [-h, h, h],
        ],
        triangles: vec![
            [0, 2, 1],
            [0, 3, 2],
            [4, 5, 6],
            [4, 6, 7],
            [0, 1, 5],
            [0, 5, 4],
            [2, 3, 7],
            [2, 7, 6],
            [1, 2, 6],
            [1, 6, 5],
            [3, 0, 4],
            [3, 4, 7],
        ],
    }
}

/// A row of `n` well-separated cubes (no interference) — the common case.
fn assembly_of(n: u32) -> Assembly {
    let mut assembly = Assembly::new(InstanceId(0));
    for k in 0..n {
        let mut instance = Instance::new(InstanceId(k), format!("part_{k}"), cube(0.4));
        instance.translation = [f64::from(k) * 2.0, 0.0, 0.0];
        assembly.add_instance(instance);
    }
    assembly
}

fn bench_interference(c: &mut Criterion) {
    let assembly = assembly_of(50);
    c.bench_function("interference_report/50_parts", |b| {
        b.iter(|| black_box(black_box(&assembly).interference_report()))
    });
}

criterion_group!(benches, bench_interference);
criterion_main!(benches);
