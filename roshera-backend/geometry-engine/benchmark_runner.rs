// Standalone benchmark runner for geometry-engine math module
use std::time::Instant;
use geometry_engine::math::{Vector3, Point3, Matrix4, Matrix3};

fn main() {
    println!("🚀 ROSHERA GEOMETRY ENGINE - MATH MODULE BENCHMARKS");
    println!("==================================================");
    println!("Running comprehensive performance tests...\n");

    // Warmup
    warmup();

    // Run benchmarks
    benchmark_vector3();
    benchmark_matrix4();
    benchmark_point3();
    
    println!("\n✅ Benchmark complete!");
}

fn warmup() {
    println!("🔥 Warming up...");
    let v1 = Vector3::new(1.0, 2.0, 3.0);
    let v2 = Vector3::new(4.0, 5.0, 6.0);
    
    for _ in 0..100_000 {
        std::hint::black_box(v1.dot(&v2));
        std::hint::black_box(v1.cross(&v2));
    }
}

fn benchmark_vector3() {
    println!("\n📊 VECTOR3 OPERATIONS");
    println!("====================");
    
    let v1 = Vector3::new(1.234, 5.678, 9.012);
    let v2 = Vector3::new(3.456, 7.890, 1.234);
    let iterations = 1_000_000;
    
    // Dot product
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.dot(&v2));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Dot Product:     {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Cross product
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.cross(&v2));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Cross Product:   {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Addition
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1 + v2);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Addition:        {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Normalization
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.normalize());
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Normalize:       {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Magnitude
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(v1.magnitude());
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Magnitude:       {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
}

fn benchmark_matrix4() {
    println!("\n📊 MATRIX4 OPERATIONS");
    println!("====================");
    
    let m1 = Matrix4::from_scale(2.0);
    let m2 = Matrix4::rotation_x(0.5);
    let p = Point3::new(1.0, 2.0, 3.0);
    let v = Vector3::new(1.0, 0.0, 0.0);
    let iterations = 1_000_000;
    
    // Matrix multiplication
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1 * m2);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Matrix Multiply: {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Transform point
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1.transform_point(&p));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Transform Point: {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Transform vector
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1.transform_vector(&v));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Transform Vec:   {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Determinant
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(m1.determinant());
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Determinant:     {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
}

fn benchmark_point3() {
    println!("\n📊 POINT3 OPERATIONS");
    println!("===================");
    
    let p1 = Point3::new(1.234, 5.678, 9.012);
    let p2 = Point3::new(3.456, 7.890, 1.234);
    let v = Vector3::new(1.0, 0.0, 0.0);
    let iterations = 1_000_000;
    
    // Distance
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(p1.distance(&p2));
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Distance:        {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Point + Vector
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(p1 + v);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Point + Vector:  {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
    
    // Point - Point
    let start = Instant::now();
    for _ in 0..iterations {
        std::hint::black_box(p1 - p2);
    }
    let duration = start.elapsed();
    let ns_per_op = duration.as_nanos() as f64 / iterations as f64;
    println!("Point - Point:   {:.1} ns/op ({:.0} M ops/sec)", 
             ns_per_op, 1000.0 / ns_per_op);
}