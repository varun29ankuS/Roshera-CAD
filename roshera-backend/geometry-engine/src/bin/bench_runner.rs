use geometry_engine::math::{Matrix4, Point3, Vector3};
use geometry_engine::primitives::topology_builder::TopologyBuilder;
use geometry_engine::BRepModel;
use serde_json::json;
use std::time::{Duration, Instant};

fn benchmark<F>(name: &str, mut f: F, iterations: u32) -> f64
where
    F: FnMut(),
{
    // Warmup
    for _ in 0..100 {
        f();
    }

    // Actual benchmark
    let start = Instant::now();
    for _ in 0..iterations {
        f();
    }
    let elapsed = start.elapsed();

    // Return nanoseconds per iteration
    elapsed.as_nanos() as f64 / iterations as f64
}

fn main() {
    let mut results = Vec::new();

    // Vector operations
    let v1 = Vector3::new(1.234, 5.678, 9.012);
    let v2 = Vector3::new(3.456, 7.890, 1.234);

    let ns = benchmark(
        "vector_operations/dot_product",
        || {
            std::hint::black_box(v1.dot(&v2));
        },
        100000,
    );
    results.push(json!({
        "name": "vector_operations/dot_product",
        "value": ns,
        "unit": "ns/iter"
    }));

    let ns = benchmark(
        "vector_operations/cross_product",
        || {
            std::hint::black_box(v1.cross(&v2));
        },
        100000,
    );
    results.push(json!({
        "name": "vector_operations/cross_product",
        "value": ns,
        "unit": "ns/iter"
    }));

    // Matrix operations
    let m1 = Matrix4::identity();
    let m2 = Matrix4::from_translation(&Vector3::new(1.0, 2.0, 3.0));

    let ns = benchmark(
        "matrix_operations/multiply",
        || {
            std::hint::black_box(m1 * m2);
        },
        100000,
    );
    results.push(json!({
        "name": "matrix_operations/multiply",
        "value": ns,
        "unit": "ns/iter"
    }));

    // Primitive creation
    let ns = benchmark(
        "primitive_creation/create_box",
        || {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            std::hint::black_box(builder.create_box_3d(5.0, 3.0, 2.0));
        },
        1000,
    );
    results.push(json!({
        "name": "primitive_creation/create_box",
        "value": ns,
        "unit": "ns/iter"
    }));

    let ns = benchmark(
        "primitive_creation/create_sphere",
        || {
            let mut model = BRepModel::new();
            let mut builder = TopologyBuilder::new(&mut model);
            std::hint::black_box(builder.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 5.0));
        },
        1000,
    );
    results.push(json!({
        "name": "primitive_creation/create_sphere",
        "value": ns,
        "unit": "ns/iter"
    }));

    // Output JSON in the format expected by github-action-benchmark
    // For 'customSmallerIsBetter' tool, it expects an array at the root level
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}
