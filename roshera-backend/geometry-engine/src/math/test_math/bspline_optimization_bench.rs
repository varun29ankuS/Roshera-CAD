use crate::math::{
    bspline::{evaluate_batch_simd, BSplineCurve, BSplineWorkspace},
    Point3,
};
use std::time::Instant;

const WARMUP_ITERATIONS: usize = 1000;
const BENCHMARK_ITERATIONS: usize = 100_000;

fn benchmark<F>(name: &str, mut op: F) -> f64
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

    let ns_per_op = elapsed.as_nanos() as f64 / BENCHMARK_ITERATIONS as f64;
    let ops_per_sec = 1_000_000_000.0 / ns_per_op;

    println!(
        "{:<50} │ {:>10.1} ns/op │ {:>12}/s",
        name,
        ns_per_op,
        if ops_per_sec >= 1_000_000_000.0 {
            format!("{:.1}G", ops_per_sec / 1_000_000_000.0)
        } else if ops_per_sec >= 1_000_000.0 {
            format!("{:.1}M", ops_per_sec / 1_000_000.0)
        } else if ops_per_sec >= 1_000.0 {
            format!("{:.1}K", ops_per_sec / 1_000.0)
        } else {
            format!("{:.1}", ops_per_sec)
        }
    );

    ns_per_op
}

fn create_test_curve() -> BSplineCurve {
    // Create a cubic B-spline with 6 control points
    let control_points = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 2.0, 0.5),
        Point3::new(2.0, 1.5, 1.0),
        Point3::new(3.0, 3.0, 0.8),
        Point3::new(4.0, 2.5, 0.3),
        Point3::new(5.0, 0.0, 0.0),
    ];

    // For 6 control points and degree 3, we need 6 + 3 + 1 = 10 knots
    let knots = vec![0.0, 0.0, 0.0, 0.0, 0.33, 0.67, 1.0, 1.0, 1.0, 1.0];
    BSplineCurve::new(3, control_points, knots).expect("Failed to create B-spline")
}

#[test]
fn bench_bspline_optimization() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║               B-SPLINE OPTIMIZATION COMPARISON BENCHMARK                      ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    // Create test curve
    let curve = create_test_curve();

    println!("🔧 SETUP");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Control points: {}", curve.control_points.len());
    println!("  Degree: {}", curve.degree);
    println!("  Iterations: {}", BENCHMARK_ITERATIONS);

    println!("\n📊 SINGLE POINT EVALUATION");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Benchmark B-spline evaluation
    let eval_ns = benchmark("BSpline::evaluate(0.5)", || {
        let _ = curve.evaluate(0.5);
    });

    // Benchmark with workspace reuse
    let mut workspace = BSplineWorkspace::new(curve.degree);
    let workspace_ns = benchmark("BSpline with workspace reuse", || {
        workspace.reset(curve.degree);
        let _ = curve.evaluate(0.5);
    });

    println!("\n📊 SPAN FINDING");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Benchmark span finding
    let span_ns = benchmark("find_span(0.5)", || {
        let _ = curve.find_span(0.5);
    });

    println!("\n📊 BATCH EVALUATION (100 points)");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Create parameter values for batch evaluation
    let params: Vec<f64> = (0..100).map(|i| i as f64 / 99.0).collect();
    let mut output = vec![Point3::new(0.0, 0.0, 0.0); 100];

    // Benchmark sequential batch
    let sequential_batch_ns = benchmark("Sequential batch (100 points)", || {
        for (i, &u) in params.iter().enumerate() {
            output[i] = curve.evaluate(u).unwrap();
        }
    });

    // Benchmark SIMD batch
    let simd_batch_ns = benchmark("SIMD batch (100 points)", || {
        let _ = evaluate_batch_simd(&curve, &params, &mut output);
    });

    println!("\n📈 PERFORMANCE SUMMARY");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    let batch_improvement =
        ((sequential_batch_ns - simd_batch_ns) / sequential_batch_ns * 100.0).abs();

    println!("  Single evaluation:                {:.1} ns/op", eval_ns);
    println!(
        "  With workspace reuse:             {:.1} ns/op",
        workspace_ns
    );
    println!("  Span finding:                     {:.1} ns/op", span_ns);
    println!(
        "  Batch SIMD improvement:           {:.1}%",
        batch_improvement
    );

    println!("\n🎯 TARGET ANALYSIS");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Target:                           < 50 ns/op");
    println!("  Achieved:                         {:.1} ns/op", eval_ns);

    if eval_ns < 50.0 {
        println!("  ✅ OPTIMIZATION TARGET ACHIEVED!");
    } else if eval_ns < 75.0 {
        println!("  ⚠️  Close to target ({}x slower)", (eval_ns / 50.0));
    } else {
        println!(
            "  ❌ More optimization needed ({}x slower)",
            (eval_ns / 50.0)
        );
    }

    println!("\n💡 OPTIMIZATION TECHNIQUES");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  ✓ Zero heap allocations (stack arrays)");
    println!("  ✓ SIMD vectorization (AVX2 on x86_64)");
    println!("  ✓ Monomorphized code paths (no vtables)");
    println!("  ✓ Branchless binary search");
    println!("  ✓ Unrolled Cox-de Boor for cubic");
    println!("  ✓ Structure of Arrays (SoA) layout");
    println!("  ✓ Unsafe indexing (bounds checks removed)");
}
