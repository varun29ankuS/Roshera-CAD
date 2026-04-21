use geometry_engine::math::{
    nurbs::{NurbsCurve, NurbsSurface},
    Point3,
};
use std::time::{Duration, Instant};

const WARMUP_ITERATIONS: usize = 1000;
const BENCHMARK_ITERATIONS: usize = 100_000;

#[derive(Debug)]
struct BenchmarkResult {
    operation: String,
    total_time: Duration,
    iterations: usize,
    ns_per_op: f64,
    ops_per_sec: f64,
}

impl BenchmarkResult {
    fn new(operation: &str, total_time: Duration, iterations: usize) -> Self {
        let ns_per_op = total_time.as_nanos() as f64 / iterations as f64;
        let ops_per_sec = 1_000_000_000.0 / ns_per_op;

        Self {
            operation: operation.to_string(),
            total_time,
            iterations,
            ns_per_op,
            ops_per_sec,
        }
    }

    fn print(&self) {
        println!(
            "{:<40} │ {:>10.1} ns/op │ {:>12} ops/sec",
            self.operation,
            self.ns_per_op,
            format_ops_per_sec(self.ops_per_sec)
        );
    }
}

fn format_ops_per_sec(ops: f64) -> String {
    if ops >= 1_000_000_000.0 {
        format!("{:.1}G", ops / 1_000_000_000.0)
    } else if ops >= 1_000_000.0 {
        format!("{:.1}M", ops / 1_000_000.0)
    } else if ops >= 1_000.0 {
        format!("{:.1}K", ops / 1_000.0)
    } else {
        format!("{:.1}", ops)
    }
}

fn benchmark<F>(name: &str, mut op: F) -> BenchmarkResult
where
    F: FnMut(),
{
    // Warmup
    for _ in 0..WARMUP_ITERATIONS {
        std::hint::black_box(op());
    }

    // Actual benchmark
    let start = Instant::now();
    for _ in 0..BENCHMARK_ITERATIONS {
        std::hint::black_box(op());
    }
    let elapsed = start.elapsed();

    BenchmarkResult::new(name, elapsed, BENCHMARK_ITERATIONS)
}

fn create_test_nurbs_curve() -> NurbsCurve {
    // Create a simple NURBS curve (not circular arc which requires special construction)
    // Quadratic NURBS curve with 5 control points
    let control_points = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 2.0, 0.0),
        Point3::new(3.0, 3.0, 0.0),
        Point3::new(5.0, 2.0, 0.0),
        Point3::new(6.0, 0.0, 0.0),
    ];

    // Weights (uniform for simple curve)
    let weights = vec![1.0, 1.0, 1.0, 1.0, 1.0];

    // For 5 control points and degree 2, we need 5 + 2 + 1 = 8 knots
    let knots = vec![0.0, 0.0, 0.0, 0.5, 0.75, 1.0, 1.0, 1.0];

    NurbsCurve::new(control_points, weights, knots, 2).expect("Failed to create NURBS curve")
}

fn create_test_nurbs_surface() -> NurbsSurface {
    // Create a simple bilinear NURBS surface (degree 1x1)
    let control_points = vec![
        vec![
            Point3::new(0.0, 0.0, 0.0),
            Point3::new(1.0, 0.0, 0.0),
            Point3::new(2.0, 0.0, 0.0),
        ],
        vec![
            Point3::new(0.0, 1.0, 0.5),
            Point3::new(1.0, 1.0, 1.0),
            Point3::new(2.0, 1.0, 0.5),
        ],
        vec![
            Point3::new(0.0, 2.0, 0.0),
            Point3::new(1.0, 2.0, 0.0),
            Point3::new(2.0, 2.0, 0.0),
        ],
    ];

    let weights = vec![
        vec![1.0, 1.0, 1.0],
        vec![1.0, 2.0, 1.0], // Middle row has higher weight
        vec![1.0, 1.0, 1.0],
    ];

    let knots_u = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let knots_v = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];

    NurbsSurface::new(control_points, weights, knots_u, knots_v, 2, 2)
        .expect("Failed to create NURBS surface")
}

fn main() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                      NURBS/B-SPLINE PERFORMANCE BENCHMARKS                    ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    // Create test data
    let nurbs = create_test_nurbs_curve();
    let surface = create_test_nurbs_surface();

    println!("📊 NURBS CURVE OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    let mut nurbs_results = Vec::new();

    nurbs_results.push(benchmark("NURBS::evaluate (single point)", || {
        nurbs.evaluate(0.5);
    }));

    nurbs_results.push(benchmark("NURBS::evaluate_derivatives (1st)", || {
        nurbs.evaluate_derivatives(0.5, 1);
    }));

    nurbs_results.push(benchmark("NURBS::evaluate_derivatives (2nd)", || {
        nurbs.evaluate_derivatives(0.5, 2);
    }));

    // Test multiple evaluations (simulating tessellation)
    let params: Vec<f64> = (0..100).map(|i| i as f64 / 99.0).collect();
    nurbs_results.push(benchmark("NURBS::evaluate (100 points batch)", || {
        for &u in &params {
            nurbs.evaluate(u);
        }
    }));

    for result in &nurbs_results {
        result.print();
    }

    println!("\n🚀 SIMD-OPTIMIZED NURBS OPERATIONS (Target: <100ns)");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    let mut simd_results = Vec::new();

    simd_results.push(benchmark("NURBS::evaluate_simd (single point)", || {
        nurbs.evaluate_simd(0.5);
    }));

    simd_results.push(benchmark("NURBS::evaluate_batch_simd (100 points)", || {
        nurbs.evaluate_batch_simd(&params);
    }));

    simd_results.push(benchmark("NURBS::evaluate_derivatives_simd (1st)", || {
        nurbs.evaluate_derivatives_simd(0.5, 1);
    }));

    simd_results.push(benchmark("NURBS::evaluate_derivatives_simd (2nd)", || {
        nurbs.evaluate_derivatives_simd(0.5, 2);
    }));

    for result in &simd_results {
        result.print();
    }

    // Performance comparison
    println!("\n📈 PERFORMANCE COMPARISON");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    println!("Single Point Evaluation:");
    let regular_single = nurbs_results[0].ns_per_op;
    let simd_single = simd_results[0].ns_per_op;
    let speedup = regular_single / simd_single;

    println!("  Regular:              {:>10.1} ns/op", regular_single);
    println!("  SIMD:                 {:>10.1} ns/op", simd_single);
    println!("  Speedup:              {:>10.1}x", speedup);
    println!(
        "  Target achieved:      {}",
        if simd_single < 100.0 {
            "✅ YES"
        } else {
            "❌ NO"
        }
    );

    println!("\n1st Derivative:");
    let regular_d1 = nurbs_results[1].ns_per_op;
    let simd_d1 = simd_results[2].ns_per_op;
    let d1_speedup = regular_d1 / simd_d1;

    println!("  Regular:              {:>10.1} ns/op", regular_d1);
    println!("  SIMD:                 {:>10.1} ns/op", simd_d1);
    println!("  Speedup:              {:>10.1}x", d1_speedup);

    println!("\n2nd Derivative:");
    let regular_d2 = nurbs_results[2].ns_per_op;
    let simd_d2 = simd_results[3].ns_per_op;
    let d2_speedup = regular_d2 / simd_d2;

    println!("  Regular:              {:>10.1} ns/op", regular_d2);
    println!("  SIMD:                 {:>10.1} ns/op", simd_d2);
    println!("  Speedup:              {:>10.1}x", d2_speedup);

    println!("\nBatch Processing (100 points):");
    let regular_batch = params.len() as f64 * regular_single;
    let simd_batch = simd_results[1].ns_per_op;
    let batch_speedup = regular_batch / simd_batch;

    println!("  Regular:              {:>10.1} ns/op", regular_batch);
    println!("  SIMD:                 {:>10.1} ns/op", simd_batch);
    println!("  Speedup:              {:>10.1}x", batch_speedup);

    println!("\n📊 NURBS SURFACE OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    let mut surface_results = Vec::new();

    surface_results.push(benchmark("NurbsSurface::evaluate (single point)", || {
        surface.evaluate(0.5, 0.5);
    }));

    surface_results.push(benchmark("NurbsSurface::evaluate_derivatives", || {
        surface.evaluate_derivatives(0.5, 0.5, 1, 1);
    }));

    // Grid evaluation (common for tessellation)
    let grid_size = 10;
    let grid_params: Vec<(f64, f64)> = (0..grid_size)
        .flat_map(|i| {
            (0..grid_size).map(move |j| {
                (
                    i as f64 / (grid_size - 1) as f64,
                    j as f64 / (grid_size - 1) as f64,
                )
            })
        })
        .collect();

    surface_results.push(benchmark("NurbsSurface::evaluate (10x10 grid)", || {
        for &(u, v) in &grid_params {
            surface.evaluate(u, v);
        }
    }));

    for result in &surface_results {
        result.print();
    }

    println!("\n📊 MEMORY AND CACHE ANALYSIS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Memory sizes
    println!("NurbsCurve size: {} bytes", std::mem::size_of_val(&nurbs));
    println!(
        "NurbsSurface size: {} bytes",
        std::mem::size_of_val(&surface)
    );

    // Cache behavior test
    let cache_test_params: Vec<f64> = (0..1000).map(|i| i as f64 / 999.0).collect();

    // Sequential access
    let start = Instant::now();
    for &u in &cache_test_params {
        nurbs.evaluate(u);
    }
    let sequential_time = start.elapsed();

    // Random access
    let mut random_params = cache_test_params.clone();
    // Simple shuffle implementation without rand crate
    // Just reverse the order for random-like access pattern
    random_params.reverse();

    let start = Instant::now();
    for &u in &random_params {
        nurbs.evaluate(u);
    }
    let random_time = start.elapsed();

    println!("\nCache Performance (1000 evaluations):");
    println!(
        "Sequential access: {:.2} ms",
        sequential_time.as_secs_f64() * 1000.0
    );
    println!(
        "Random access: {:.2} ms",
        random_time.as_secs_f64() * 1000.0
    );
    println!(
        "Cache penalty: {:.1}%",
        (random_time.as_secs_f64() / sequential_time.as_secs_f64() - 1.0) * 100.0
    );

    println!("\n🎯 PERFORMANCE SUMMARY");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Find fastest and slowest operations
    let mut all_results = nurbs_results;
    all_results.extend(surface_results);

    all_results.sort_by(|a, b| a.ns_per_op.partial_cmp(&b.ns_per_op).unwrap());

    println!("\n✅ FASTEST OPERATIONS:");
    for result in all_results.iter().take(3) {
        println!("  {}: {:.1} ns/op", result.operation, result.ns_per_op);
    }

    println!("\n⚠️  SLOWEST OPERATIONS:");
    for result in all_results.iter().rev().take(3) {
        println!("  {}: {:.1} ns/op", result.operation, result.ns_per_op);
    }

    println!("\n📈 TARGET COMPARISON:");
    println!("  Industry target (NURBS evaluation): < 100 ns/op");
    println!("  Industry target (B-spline evaluation): < 50 ns/op");

    // Check if we meet targets
    let nurbs_eval = all_results
        .iter()
        .find(|r| r.operation.contains("NURBS::evaluate (single point)"))
        .unwrap();

    if nurbs_eval.ns_per_op < 100.0 {
        println!(
            "  ✅ NURBS evaluation: {:.1} ns/op - MEETS TARGET!",
            nurbs_eval.ns_per_op
        );
    } else {
        println!(
            "  ❌ NURBS evaluation: {:.1} ns/op - NEEDS OPTIMIZATION",
            nurbs_eval.ns_per_op
        );
    }
}
