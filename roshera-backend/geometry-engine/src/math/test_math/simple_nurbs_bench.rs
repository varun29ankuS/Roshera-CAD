use crate::math::{bspline::BSplineCurve, Point3};
use std::time::{Duration, Instant};

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
        "{:<40} │ {:>10.1} ns/op │ {:>12}/s",
        name,
        ns_per_op,
        if ops_per_sec >= 1_000_000_000.0 {
            format!("{:.1}G", ops_per_sec / 1_000_000_000.0)
        } else if ops_per_sec >= 1_000_000.0 {
            format!("{:.1}M", ops_per_sec / 1_000_000.0)
        } else {
            format!("{:.1}K", ops_per_sec / 1_000.0)
        }
    );

    ns_per_op
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

fn main() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                           B-SPLINE PERFORMANCE TEST                           ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    let curve = create_test_bspline_curve();

    println!("📊 B-SPLINE CURVE OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    let eval_ns = benchmark("BSpline::evaluate (single point)", || {
        let _ = curve.evaluate(0.5);
    });

    benchmark("BSpline::evaluate_derivatives (1st)", || {
        let _ = curve.evaluate_derivatives(0.5, 1);
    });

    benchmark("BSpline::evaluate_derivatives (2nd)", || {
        let _ = curve.evaluate_derivatives(0.5, 2);
    });

    benchmark("BSpline::find_span", || {
        let _ = curve.find_span(0.5);
    });

    // Test multiple evaluations (simulating tessellation)
    let params: Vec<f64> = (0..100).map(|i| i as f64 / 99.0).collect();
    let batch_ns = benchmark("BSpline::evaluate (100 points batch)", || {
        for &u in &params {
            let _ = curve.evaluate(u);
        }
    });

    println!("\n📈 PERFORMANCE SUMMARY");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Single evaluation: {:.1} ns/op", eval_ns);
    println!(
        "  Batch evaluation:  {:.1} ns/op per point",
        batch_ns / 100.0
    );
    println!("  Industry target:   < 50 ns/op");

    if eval_ns < 50.0 {
        println!("  ✅ B-spline evaluation: MEETS TARGET!");
    } else {
        println!("  ❌ B-spline evaluation: NEEDS OPTIMIZATION");
    }
}
