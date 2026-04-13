use crate::math::{
    bspline::BSplineCurve,
    consts,
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

fn create_test_bspline_curve() -> BSplineCurve {
    // Create a cubic B-spline curve (degree 3)
    let control_points = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 2.0, 0.0),
        Point3::new(3.0, 3.0, 0.0),
        Point3::new(4.0, 2.0, 0.0),
        Point3::new(5.0, 0.0, 0.0),
        Point3::new(6.0, -1.0, 0.0),
    ];

    // For 6 control points and degree 3, we need 6 + 3 + 1 = 10 knots
    let knots = vec![0.0, 0.0, 0.0, 0.0, 0.33, 0.67, 1.0, 1.0, 1.0, 1.0];

    BSplineCurve::new(3, control_points, knots).expect("Failed to create B-spline")
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
    let knots = vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0];

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
    let bspline = create_test_bspline_curve();
    let nurbs = create_test_nurbs_curve();
    let surface = create_test_nurbs_surface();

    println!("📊 B-SPLINE CURVE OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // B-spline benchmarks
    let mut results = Vec::new();

    results.push(benchmark("BSpline::evaluate (single point)", || {
        bspline.evaluate(0.5);
    }));

    results.push(benchmark("BSpline::evaluate_derivatives (1st)", || {
        bspline.evaluate_derivatives(0.5, 1);
    }));

    results.push(benchmark("BSpline::evaluate_derivatives (2nd)", || {
        bspline.evaluate_derivatives(0.5, 2);
    }));

    results.push(benchmark("BSpline::find_span", || {
        bspline.find_span(0.5);
    }));

    results.push(benchmark("BSpline::basis_functions", || {
        let span = 5;
        bspline.basis_functions(span, 0.5);
    }));

    for result in &results {
        result.print();
    }

    println!("\n📊 NURBS CURVE OPERATIONS");
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
    println!(
        "BSplineCurve size: {} bytes",
        std::mem::size_of_val(&bspline)
    );
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
    let mut all_results = results;
    all_results.extend(nurbs_results);
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
    let bspline_eval = all_results
        .iter()
        .find(|r| r.operation.contains("BSpline::evaluate (single point)"))
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

    if bspline_eval.ns_per_op < 50.0 {
        println!(
            "  ✅ B-spline evaluation: {:.1} ns/op - MEETS TARGET!",
            bspline_eval.ns_per_op
        );
    } else {
        println!(
            "  ❌ B-spline evaluation: {:.1} ns/op - NEEDS OPTIMIZATION",
            bspline_eval.ns_per_op
        );
    }
}
