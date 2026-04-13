// Comprehensive benchmark test - add this to src/math/test_math/bench_verification.rs
// Run with: cargo test --release bench_verification -- --nocapture

use crate::math::*;
use std::time::Instant;

// Operation counts to test
const OPERATION_COUNTS: &[usize] = &[1, 10, 100, 1000];
const WARMUP_ITERATIONS: usize = 100;

// Results storage
#[derive(Debug, Clone)]
struct BenchmarkResult {
    operation: String,
    measurements: Vec<TimingResult>,
}

#[derive(Debug, Clone)]
struct TimingResult {
    op_count: usize,
    total_ns: f64,
    ns_per_op: f64,
    ops_per_sec: f64,
}

// Helper function to prevent compiler optimization
fn black_box_return<T>(mut val: T) -> T {
    let ret = unsafe { std::ptr::read_volatile(&val) };
    std::mem::forget(val);
    ret
}

// Measure operation performance at different scales
fn benchmark_scaled<F, R>(name: &str, mut op: F) -> BenchmarkResult
where
    F: FnMut() -> R,
{
    let mut measurements = Vec::new();

    for &op_count in OPERATION_COUNTS {
        // Warmup
        for _ in 0..WARMUP_ITERATIONS {
            std::hint::black_box(op());
        }

        // Take multiple samples
        let mut timings = Vec::with_capacity(5);

        for _ in 0..5 {
            let start = Instant::now();
            for _ in 0..op_count {
                std::hint::black_box(op());
            }
            let elapsed = start.elapsed();
            timings.push(elapsed.as_nanos() as f64);
        }

        // Use median timing
        timings.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let total_ns = timings[timings.len() / 2];
        let ns_per_op = total_ns / op_count as f64;
        let ops_per_sec = 1e9 / ns_per_op;

        measurements.push(TimingResult {
            op_count,
            total_ns,
            ns_per_op,
            ops_per_sec,
        });
    }

    BenchmarkResult {
        operation: name.to_string(),
        measurements,
    }
}

// Print results in a formatted table
fn print_results_table(category: &str, results: &[BenchmarkResult]) {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║ {:^76} ║", category);
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    println!(
        "\n{:<35} │ {:>12} │ {:>12} │ {:>12} │ {:>12}",
        "Operation", "1 op", "10 ops", "100 ops", "1000 ops"
    );
    println!(
        "{:─<35}─┼─{:─>12}─┼─{:─>12}─┼─{:─>12}─┼─{:─>12}",
        "", "", "", "", ""
    );

    for result in results {
        print!("{:<35} │", result.operation);
        for measurement in &result.measurements {
            print!(" {:>10.1}ns │", measurement.ns_per_op);
        }
        println!();

        // Also print ops/sec
        print!("{:<35} │", "");
        for measurement in &result.measurements {
            if measurement.ops_per_sec > 1e9 {
                print!(" {:>9.1}G/s │", measurement.ops_per_sec / 1e9);
            } else if measurement.ops_per_sec > 1e6 {
                print!(" {:>9.1}M/s │", measurement.ops_per_sec / 1e6);
            } else if measurement.ops_per_sec > 1e3 {
                print!(" {:>9.1}K/s │", measurement.ops_per_sec / 1e3);
            } else {
                print!(" {:>9.1}/s │", measurement.ops_per_sec);
            }
        }
        println!();
        println!(
            "{:─<35}─┼─{:─>12}─┼─{:─>12}─┼─{:─>12}─┼─{:─>12}",
            "", "", "", "", ""
        );
    }
}

// Generate performance report
fn generate_report(all_results: Vec<(&str, Vec<BenchmarkResult>)>) {
    println!(
        "\n\n╔══════════════════════════════════════════════════════════════════════════════╗"
    );
    println!("║                            PERFORMANCE REPORT                                 ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    // System info
    println!("\n=== System Information ===");
    println!("OS: {} {}", std::env::consts::OS, std::env::consts::ARCH);

    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    println!("CPU Cores: {}", cpu_count);

    #[cfg(debug_assertions)]
    println!("Build Mode: Debug (WARNING: Run in release mode for accurate results!)");
    #[cfg(not(debug_assertions))]
    println!("Build Mode: Release (Optimized)");

    // Remove chrono dependency - just use simple timestamp
    println!("Test run completed");

    // Performance scaling analysis
    println!("\n=== Performance Scaling Analysis ===");
    println!("Analyzing how performance scales from 1 to 1000 operations...\n");

    for (category, results) in &all_results {
        let mut scaling_good = 0;
        let mut scaling_total = 0;

        for result in results {
            if result.measurements.len() >= 2 {
                let ratio_1_to_1000 =
                    result.measurements[3].ns_per_op / result.measurements[0].ns_per_op;
                scaling_total += 1;
                if ratio_1_to_1000 < 1.5 {
                    scaling_good += 1;
                }
            }
        }

        let percentage = if scaling_total > 0 {
            (scaling_good as f64 / scaling_total as f64 * 100.0) as u32
        } else {
            0
        };

        println!("{}: {}% of operations scale well", category, percentage);
    }

    // Find fastest and slowest operations
    println!("\n=== Performance Extremes ===");

    let mut all_ops: Vec<(&str, &str, f64)> = Vec::new();
    for (category, results) in &all_results {
        for result in results {
            if let Some(measurement) = result.measurements.last() {
                all_ops.push((category, &result.operation, measurement.ns_per_op));
            }
        }
    }

    all_ops.sort_by(|a, b| a.2.partial_cmp(&b.2).unwrap());

    println!("\nFastest Operations (at 1000 ops):");
    for i in 0..5.min(all_ops.len()) {
        println!(
            "  {}: {} - {:.1} ns/op",
            all_ops[i].0, all_ops[i].1, all_ops[i].2
        );
    }

    println!("\nSlowest Operations (at 1000 ops):");
    for i in (all_ops.len().saturating_sub(5))..all_ops.len() {
        println!(
            "  {}: {} - {:.1} ns/op",
            all_ops[i].0, all_ops[i].1, all_ops[i].2
        );
    }

    // Memory efficiency
    println!("\n=== Memory Layout Efficiency ===");
    println!("Structure           Size    Alignment   Efficiency");
    println!("─────────────────────────────────────────────────");

    let types = vec![
        (
            "Vector3",
            std::mem::size_of::<Vector3>(),
            std::mem::align_of::<Vector3>(),
        ),
        (
            "Vector2",
            std::mem::size_of::<crate::math::vector2::Vector2>(),
            std::mem::align_of::<crate::math::vector2::Vector2>(),
        ),
        (
            "Matrix4",
            std::mem::size_of::<Matrix4>(),
            std::mem::align_of::<Matrix4>(),
        ),
        (
            "Matrix3",
            std::mem::size_of::<Matrix3>(),
            std::mem::align_of::<Matrix3>(),
        ),
    ];

    for (name, size, align) in types {
        let efficiency = if size % align == 0 {
            "Optimal"
        } else {
            "Suboptimal"
        };
        println!(
            "{:<15} {:>6} bytes {:>8} bytes   {}",
            name, size, align, efficiency
        );
    }
}

#[test]
fn bench_vector_ops() {
    let mut results = Vec::new();

    let v1 = Vector3::new(1.234, 5.678, 9.012);
    let v2 = Vector3::new(3.456, 7.890, 1.234);
    let v3 = Vector3::new(-2.345, 6.789, -3.456);

    // Basic arithmetic
    results.push(benchmark_scaled("Vector3::add", || {
        black_box_return(v1 + v2)
    }));
    results.push(benchmark_scaled("Vector3::sub", || {
        black_box_return(v1 - v2)
    }));
    results.push(benchmark_scaled("Vector3::mul scalar", || {
        black_box_return(v1 * 2.5)
    }));
    results.push(benchmark_scaled("Vector3::div scalar", || {
        black_box_return(v1 / 2.5)
    }));
    results.push(benchmark_scaled("Vector3::neg", || black_box_return(-v1)));

    // Core operations
    results.push(benchmark_scaled("Vector3::dot", || {
        black_box_return(v1.dot(&v2))
    }));
    results.push(benchmark_scaled("Vector3::cross", || {
        black_box_return(v1.cross(&v2))
    }));
    results.push(benchmark_scaled("Vector3::magnitude", || {
        black_box_return(v1.magnitude())
    }));
    results.push(benchmark_scaled("Vector3::magnitude_squared", || {
        black_box_return(v1.magnitude_squared())
    }));
    results.push(benchmark_scaled("Vector3::normalize", || {
        black_box_return(v1.normalize())
    }));

    // Distance and angle
    results.push(benchmark_scaled("Vector3::distance", || {
        black_box_return(v1.distance(&v2))
    }));
    results.push(benchmark_scaled("Vector3::angle", || {
        black_box_return(v1.angle(&v2))
    }));

    // Interpolation and projection
    results.push(benchmark_scaled("Vector3::lerp", || {
        black_box_return(v1.lerp(&v2, 0.5))
    }));
    results.push(benchmark_scaled("Vector3::project", || {
        black_box_return(v1.project(&v2))
    }));

    // Component operations
    results.push(benchmark_scaled("Vector3::abs", || {
        black_box_return(v1.abs())
    }));
    results.push(benchmark_scaled("Vector3::min", || {
        black_box_return(v1.min(&v2))
    }));
    results.push(benchmark_scaled("Vector3::max", || {
        black_box_return(v1.max(&v2))
    }));

    print_results_table("Vector3 Operations", &results);
}

#[test]
fn bench_matrix_ops() {
    let mut results = Vec::new();

    let m1 = Matrix4::rotation_x(0.5) * Matrix4::translation(1.0, 2.0, 3.0);
    let m2 = Matrix4::rotation_y(0.7) * Matrix4::scale(2.0, 3.0, 4.0);

    // Construction
    results.push(benchmark_scaled("Matrix4::identity", || {
        black_box_return(Matrix4::IDENTITY)
    }));
    results.push(benchmark_scaled("Matrix4::translation", || {
        black_box_return(Matrix4::translation(1.0, 2.0, 3.0))
    }));
    results.push(benchmark_scaled("Matrix4::rotation_x", || {
        black_box_return(Matrix4::rotation_x(0.5))
    }));
    results.push(benchmark_scaled("Matrix4::scale", || {
        black_box_return(Matrix4::scale(2.0, 3.0, 4.0))
    }));

    // Core operations
    results.push(benchmark_scaled("Matrix4::multiply", || {
        black_box_return(m1 * m2)
    }));
    results.push(benchmark_scaled("Matrix4::transpose", || {
        black_box_return(m1.transpose())
    }));
    results.push(benchmark_scaled("Matrix4::determinant", || {
        black_box_return(m1.determinant())
    }));
    results.push(benchmark_scaled("Matrix4::inverse", || {
        black_box_return(m1.inverse())
    }));

    // Transformation
    let point = Point3::new(1.0, 2.0, 3.0);
    let vector = Vector3::X;
    results.push(benchmark_scaled("Matrix4::transform_point", || {
        black_box_return(m1.transform_point(&point))
    }));
    results.push(benchmark_scaled("Matrix4::transform_vector", || {
        black_box_return(m1.transform_vector(&vector))
    }));

    print_results_table("Matrix4 Operations", &results);
}

#[test]
fn bench_all_categories() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                    ROSHERA CAD MATH MODULE BENCHMARKS                        ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    let mut all_results = Vec::new();

    // Vector3 operations
    {
        let mut results = Vec::new();
        let v1 = Vector3::new(1.234, 5.678, 9.012);
        let v2 = Vector3::new(3.456, 7.890, 1.234);

        results.push(benchmark_scaled("Vector3::dot", || {
            black_box_return(v1.dot(&v2))
        }));
        results.push(benchmark_scaled("Vector3::cross", || {
            black_box_return(v1.cross(&v2))
        }));
        results.push(benchmark_scaled("Vector3::normalize", || {
            black_box_return(v1.normalize())
        }));
        results.push(benchmark_scaled("Vector3::magnitude", || {
            black_box_return(v1.magnitude())
        }));
        results.push(benchmark_scaled("Vector3::lerp", || {
            black_box_return(v1.lerp(&v2, 0.5))
        }));

        print_results_table("Vector3 Operations", &results);
        all_results.push(("Vector3", results));
    }

    // Vector2 operations
    {
        use crate::math::vector2::Vector2;
        let mut results = Vec::new();
        let v1 = Vector2::new(1.234, 5.678);
        let v2 = Vector2::new(3.456, 7.890);

        results.push(benchmark_scaled("Vector2::dot", || {
            black_box_return(v1.dot(&v2))
        }));
        results.push(benchmark_scaled("Vector2::perp_dot", || {
            black_box_return(v1.perp_dot(&v2))
        }));
        results.push(benchmark_scaled("Vector2::normalize", || {
            black_box_return(v1.normalize())
        }));
        results.push(benchmark_scaled("Vector2::rotate", || {
            black_box_return(v1.rotate(0.5))
        }));

        print_results_table("Vector2 Operations", &results);
        all_results.push(("Vector2", results));
    }

    // Matrix operations
    {
        let mut results = Vec::new();
        let m1 = Matrix4::rotation_x(0.5);
        let m2 = Matrix4::rotation_y(0.7);
        let point = Point3::new(1.0, 2.0, 3.0);

        results.push(benchmark_scaled("Matrix4::multiply", || {
            black_box_return(m1 * m2)
        }));
        results.push(benchmark_scaled("Matrix4::transpose", || {
            black_box_return(m1.transpose())
        }));
        results.push(benchmark_scaled("Matrix4::determinant", || {
            black_box_return(m1.determinant())
        }));
        results.push(benchmark_scaled("Matrix4::transform_point", || {
            black_box_return(m1.transform_point(&point))
        }));

        print_results_table("Matrix4 Operations", &results);
        all_results.push(("Matrix4", results));
    }

    // Quaternion operations
    {
        use crate::math::quaternion::Quaternion;
        let mut results = Vec::new();
        let q1 = Quaternion::from_axis_angle(&Vector3::Y, 0.5).unwrap();
        let q2 = Quaternion::from_axis_angle(&Vector3::Z, 0.7).unwrap();
        let v = Vector3::X;

        results.push(benchmark_scaled("Quaternion::multiply", || {
            black_box_return(q1 * q2)
        }));
        results.push(benchmark_scaled("Quaternion::normalize", || {
            black_box_return(q1.normalize())
        }));
        results.push(benchmark_scaled("Quaternion::rotate_vector", || {
            black_box_return(q1.rotate_vector(&v))
        }));
        results.push(benchmark_scaled("Quaternion::slerp", || {
            black_box_return(q1.slerp(&q2, 0.5))
        }));

        print_results_table("Quaternion Operations", &results);
        all_results.push(("Quaternion", results));
    }

    // Ray operations
    {
        use crate::math::ray::Ray;
        let mut results = Vec::new();
        let ray = Ray::new(
            Point3::new(0.0, 0.0, 5.0),
            Vector3::new(0.1, 0.1, -1.0).normalize().unwrap(),
        );
        let v0 = Point3::new(0.0, 0.0, 0.0);
        let v1 = Point3::new(1.0, 0.0, 0.0);
        let v2 = Point3::new(0.0, 1.0, 0.0);

        results.push(benchmark_scaled("Ray::point_at", || {
            black_box_return(ray.point_at(5.0))
        }));
        results.push(benchmark_scaled("Ray::intersect_triangle", || {
            black_box_return(ray.intersect_triangle(&v0, &v1, &v2))
        }));
        results.push(benchmark_scaled("Ray::intersect_sphere", || {
            black_box_return(ray.intersect_sphere(&Point3::ZERO, 2.0))
        }));

        print_results_table("Ray Operations", &results);
        all_results.push(("Ray", results));
    }

    // BBox operations
    {
        use crate::math::bbox::BBox;
        let mut results = Vec::new();
        let bbox1 = BBox::new(Point3::ZERO, Point3::ONE);
        let bbox2 = BBox::new(Point3::new(0.5, 0.5, 0.5), Point3::new(1.5, 1.5, 1.5));
        let point = Point3::new(0.75, 0.75, 0.75);

        results.push(benchmark_scaled("BBox::contains_point", || {
            black_box_return(bbox1.contains_point(&point))
        }));
        results.push(benchmark_scaled("BBox::intersects", || {
            black_box_return(bbox1.intersects(&bbox2))
        }));
        results.push(benchmark_scaled("BBox::union", || {
            black_box_return(bbox1.union(&bbox2))
        }));
        results.push(benchmark_scaled("BBox::volume", || {
            black_box_return(bbox1.volume())
        }));

        print_results_table("BBox Operations", &results);
        all_results.push(("BBox", results));
    }

    // Generate comprehensive report
    generate_report(all_results);
}

#[test]
fn bench_real_world_scenarios() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                        REAL WORLD SCENARIO BENCHMARKS                        ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝");

    let mut results = Vec::new();

    // Scenario 1: Transform chain
    {
        let transform = Matrix4::translation(5.0, 0.0, 0.0)
            * Matrix4::rotation_y(std::f64::consts::PI / 4.0)
            * Matrix4::scale(2.0, 2.0, 2.0);
        let point = Point3::new(1.0, 2.0, 3.0);

        results.push(benchmark_scaled("Transform chain (3 matrices)", || {
            let t = Matrix4::translation(5.0, 0.0, 0.0)
                * Matrix4::rotation_y(std::f64::consts::PI / 4.0)
                * Matrix4::scale(2.0, 2.0, 2.0);
            black_box_return(t.transform_point(&point))
        }));
    }

    // Scenario 2: Ray-box intersection test
    {
        use crate::math::bbox::BBox;
        use crate::math::ray::Ray;
        let ray = Ray::new(Point3::new(0.0, 0.0, 5.0), Vector3::new(0.0, 0.0, -1.0));
        let bbox = BBox::new(Point3::new(-1.0, -1.0, -1.0), Point3::new(1.0, 1.0, 1.0));

        results.push(benchmark_scaled("Ray-AABB intersection", || {
            black_box_return(bbox.ray_intersection(&ray.origin, &ray.direction))
        }));
    }

    // Scenario 3: Normal calculation
    {
        let v0 = Point3::new(0.0, 0.0, 0.0);
        let v1 = Point3::new(1.0, 0.0, 0.0);
        let v2 = Point3::new(0.0, 1.0, 0.0);

        results.push(benchmark_scaled("Triangle normal calculation", || {
            let e1 = v1 - v0;
            let e2 = v2 - v0;
            let normal = e1.cross(&e2).normalize();
            black_box_return(normal)
        }));
    }

    // Scenario 4: Frustum culling check
    {
        use crate::math::bbox::BBox;
        use crate::math::plane_math::Plane;

        let planes = vec![
            Plane::new(Vector3::new(1.0, 0.0, 0.0), -10.0),
            Plane::new(Vector3::new(-1.0, 0.0, 0.0), -10.0),
            Plane::new(Vector3::new(0.0, 1.0, 0.0), -10.0),
            Plane::new(Vector3::new(0.0, -1.0, 0.0), -10.0),
            Plane::new(Vector3::new(0.0, 0.0, 1.0), -0.1),
            Plane::new(Vector3::new(0.0, 0.0, -1.0), -100.0),
        ];
        let bbox = BBox::new(Point3::new(-1.0, -1.0, -1.0), Point3::new(1.0, 1.0, 1.0));

        results.push(benchmark_scaled("Frustum culling (6 planes)", || {
            let mut inside = true;
            for plane in &planes {
                let center = bbox.center();
                let extent = bbox.size() * 0.5;
                let radius = extent.x.abs() * plane.normal.x.abs()
                    + extent.y.abs() * plane.normal.y.abs()
                    + extent.z.abs() * plane.normal.z.abs();

                if plane.distance_to_point(&center) < -radius {
                    inside = false;
                    break;
                }
            }
            black_box_return(inside)
        }));
    }

    print_results_table("Real World Scenarios", &results);
}
