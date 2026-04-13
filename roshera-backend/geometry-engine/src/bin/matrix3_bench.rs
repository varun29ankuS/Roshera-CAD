use geometry_engine::math::{Matrix3, Vector3};
use std::time::Instant;

const WARMUP_ITERATIONS: usize = 10_000;
const BENCHMARK_ITERATIONS: usize = 1_000_000;

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
    println!("{:<35} │ {:>10.1} ns/op", name, ns_per_op);
    ns_per_op
}

fn main() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                        MATRIX3 PERFORMANCE BENCHMARKS                         ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    // Create test matrices
    let m1 = Matrix3::from_cols([
        1.0, 2.0, 3.0, // Column 0
        4.0, 5.0, 6.0, // Column 1
        7.0, 8.0, 9.0, // Column 2
    ]);

    let m2 = Matrix3::from_cols([
        9.0, 8.0, 7.0, // Column 0
        6.0, 5.0, 4.0, // Column 1
        3.0, 2.0, 1.0, // Column 2
    ]);

    let rotation = Matrix3::rotation_z(std::f64::consts::PI / 4.0);
    let v = Vector3::new(1.0, 2.0, 3.0);

    println!("📊 MATRIX3 OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    benchmark("Matrix3::multiply", || {
        let _ = m1 * m2;
    });

    benchmark("Matrix3::transpose", || {
        let _ = m1.transpose();
    });

    benchmark("Matrix3::determinant", || {
        let _ = rotation.determinant();
    });

    benchmark("Matrix3::inverse", || {
        let _ = rotation.inverse();
    });

    benchmark("Matrix3::transform_vector", || {
        let _ = rotation.transform_vector(&v);
    });

    benchmark("Matrix3::rotation_x", || {
        let _ = Matrix3::rotation_x(0.5);
    });

    benchmark("Matrix3::rotation_y", || {
        let _ = Matrix3::rotation_y(0.5);
    });

    benchmark("Matrix3::rotation_z", || {
        let _ = Matrix3::rotation_z(0.5);
    });

    benchmark("Matrix3::from_axis_angle", || {
        let _ = Matrix3::from_axis_angle(&Vector3::Y, 0.5).unwrap();
    });

    benchmark("Matrix3::scale", || {
        let _ = Matrix3::scale(2.0, 3.0, 4.0);
    });

    benchmark("Matrix3::identity", || {
        let _ = Matrix3::IDENTITY;
    });

    println!("\n✅ All benchmarks complete!");
}
