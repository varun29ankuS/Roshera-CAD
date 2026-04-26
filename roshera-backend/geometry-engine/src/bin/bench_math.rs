use geometry_engine::math::{Matrix4, Point3, Vector3};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

fn main() {
    println!("🚀 ROSHERA GEOMETRY ENGINE - MATH MODULE PERFORMANCE REPORT");
    println!("==========================================================");
    println!(
        "Date: {}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    );
    println!("\nRunning comprehensive performance benchmarks...\n");

    // Warmup
    warmup();

    // Run benchmarks
    let vector_results = benchmark_vector3();
    let matrix_results = benchmark_matrix4();
    let point_results = benchmark_point3();

    // Generate report
    print_performance_report(&vector_results, &matrix_results, &point_results);
}

struct BenchmarkResult {
    operation: String,
    ns_per_op: f64,
    ops_per_sec: f64,
}

fn warmup() {
    print!("🔥 Warming up CPU caches...");
    let v1 = Vector3::new(1.0, 2.0, 3.0);
    let v2 = Vector3::new(4.0, 5.0, 6.0);

    for _ in 0..1_000_000 {
        std::hint::black_box(v1.dot(&v2));
        std::hint::black_box(v1.cross(&v2));
    }
    println!(" Done!");
}

fn benchmark_vector3() -> Vec<BenchmarkResult> {
    println!("\n📊 Benchmarking Vector3 Operations...");

    let v1 = Vector3::new(1.234, 5.678, 9.012);
    let v2 = Vector3::new(3.456, 7.890, 1.234);
    let iterations = 10_000_000;
    let mut results = Vec::new();

    // Dot product
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.dot(&v2));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Vector3::dot".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Cross product
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.cross(&v2));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Vector3::cross".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Addition
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1 + v2);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Vector3::add".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Normalization
    let start = Instant::now();
    for _ in 0..iterations {
        let _ = std::hint::black_box(v1.normalize());
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Vector3::normalize".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Magnitude
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.magnitude());
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Vector3::magnitude".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    results
}

fn benchmark_matrix4() -> Vec<BenchmarkResult> {
    println!("📊 Benchmarking Matrix4 Operations...");

    let m1 = Matrix4::from_scale(&Vector3::new(2.0, 2.0, 2.0));
    let m2 = Matrix4::rotation_x(0.5);
    let p = Point3::new(1.0, 2.0, 3.0);
    let v = Vector3::new(1.0, 0.0, 0.0);
    let iterations = 5_000_000;
    let mut results = Vec::new();

    // Matrix multiplication
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(&m1 * &m2);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Matrix4::multiply".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Transform point
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1.transform_point(&p));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Matrix4::transform_point".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Transform vector
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1.transform_vector(&v));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Matrix4::transform_vector".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Determinant
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1.determinant());
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Matrix4::determinant".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    results
}

fn benchmark_point3() -> Vec<BenchmarkResult> {
    println!("📊 Benchmarking Point3 Operations...");

    let p1 = Point3::new(1.234, 5.678, 9.012);
    let p2 = Point3::new(3.456, 7.890, 1.234);
    let v = Vector3::new(1.0, 0.0, 0.0);
    let iterations = 10_000_000;
    let mut results = Vec::new();

    // Distance
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(p1.distance(&p2));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Point3::distance".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Point + Vector
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(p1 + v);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Point3::add_vector".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    // Point - Point
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(p1 - p2);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    results.push(BenchmarkResult {
        operation: "Point3::sub_point".to_string(),
        ns_per_op,
        ops_per_sec: 1_000_000_000.0 / ns_per_op,
    });

    results
}

fn print_performance_report(
    vector_results: &[BenchmarkResult],
    matrix_results: &[BenchmarkResult],
    point_results: &[BenchmarkResult],
) {
    println!("\n");
    println!("╔══════════════════════════════════════════════════════════════════════╗");
    println!("║                    PERFORMANCE BENCHMARK RESULTS                      ║");
    println!("╚══════════════════════════════════════════════════════════════════════╝");

    println!("\n▶ VECTOR3 OPERATIONS");
    println!("┌─────────────────────────────┬────────────────┬──────────────────────┐");
    println!("│ Operation                   │ Time (ns/op)   │ Throughput          │");
    println!("├─────────────────────────────┼────────────────┼──────────────────────┤");
    for result in vector_results {
        println!(
            "│ {:<27} │ {:>14.1} │ {:>15.1} M/s │",
            result.operation,
            result.ns_per_op,
            result.ops_per_sec / 1_000_000.0
        );
    }
    println!("└─────────────────────────────┴────────────────┴──────────────────────┘");

    println!("\n▶ MATRIX4 OPERATIONS");
    println!("┌─────────────────────────────┬────────────────┬──────────────────────┐");
    println!("│ Operation                   │ Time (ns/op)   │ Throughput          │");
    println!("├─────────────────────────────┼────────────────┼──────────────────────┤");
    for result in matrix_results {
        println!(
            "│ {:<27} │ {:>14.1} │ {:>15.1} M/s │",
            result.operation,
            result.ns_per_op,
            result.ops_per_sec / 1_000_000.0
        );
    }
    println!("└─────────────────────────────┴────────────────┴──────────────────────┘");

    println!("\n▶ POINT3 OPERATIONS");
    println!("┌─────────────────────────────┬────────────────┬──────────────────────┐");
    println!("│ Operation                   │ Time (ns/op)   │ Throughput          │");
    println!("├─────────────────────────────┼────────────────┼──────────────────────┤");
    for result in point_results {
        println!(
            "│ {:<27} │ {:>14.1} │ {:>15.1} M/s │",
            result.operation,
            result.ns_per_op,
            result.ops_per_sec / 1_000_000.0
        );
    }
    println!("└─────────────────────────────┴────────────────┴──────────────────────┘");

    println!("\n✅ Benchmark complete!");
    println!("\nNOTE: These are real-world performance measurements.");
    println!("Results may vary based on CPU, system load, and thermal conditions.");
}
