use crate::math::{
    tspline::{TEdge, TFace, TSplineMesh, TVertex},
    Point3,
};
use std::collections::HashMap;
use std::time::Instant;

const WARMUP_ITERATIONS: usize = 1000;
const BENCHMARK_ITERATIONS: usize = 10_000; // Lower for T-splines as they're more complex

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
        "{:<45} │ {:>10.1} ns/op │ {:>12}/s",
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

fn create_simple_tspline_mesh() -> TSplineMesh {
    let mut mesh = TSplineMesh::new();

    // Create a simple 3x3 grid of vertices
    let mut vertex_ids = Vec::new();
    for i in 0..3 {
        for j in 0..3 {
            let id = mesh.add_vertex(
                Point3::new(i as f64, j as f64, 0.0),
                1.0,                      // uniform weight
                vec![0.0, 0.0, 1.0, 1.0], // simple knot intervals
                vec![0.0, 0.0, 1.0, 1.0],
            );
            vertex_ids.push(id);
        }
    }

    // Note: T-spline edges are created automatically when faces are added

    // Create faces (2x2 quads)
    for i in 0..2 {
        for j in 0..2 {
            let v0 = vertex_ids[i * 3 + j];
            let v1 = vertex_ids[i * 3 + j + 1];
            let v2 = vertex_ids[(i + 1) * 3 + j + 1];
            let v3 = vertex_ids[(i + 1) * 3 + j];
            let _ = mesh.add_face(vec![v0, v1, v2, v3]);
        }
    }

    mesh
}

fn create_complex_tspline_mesh() -> TSplineMesh {
    let mut mesh = TSplineMesh::new();

    // Create a more complex T-mesh with T-junctions
    // This represents a refined region in the center
    let mut vertex_map = HashMap::new();

    // Outer 5x5 grid
    for i in 0..5 {
        for j in 0..5 {
            let x = i as f64 * 0.25;
            let y = j as f64 * 0.25;
            let z = ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt() * 0.5; // Simple surface

            let id = mesh.add_vertex(
                Point3::new(x, y, z),
                1.0,
                vec![0.0, 0.25, 0.5, 0.75, 1.0],
                vec![0.0, 0.25, 0.5, 0.75, 1.0],
            );
            vertex_map.insert((i, j), id);
        }
    }

    // Add refined vertices in center (creating T-junctions)
    for i in 1..4 {
        for j in 1..4 {
            let x = i as f64 * 0.25 + 0.125;
            let y = j as f64 * 0.25 + 0.125;
            let z = ((x - 0.5).powi(2) + (y - 0.5).powi(2)).sqrt() * 0.5;

            let id = mesh.add_vertex(
                Point3::new(x, y, z),
                1.0,
                vec![0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0],
                vec![0.0, 0.125, 0.25, 0.375, 0.5, 0.625, 0.75, 0.875, 1.0],
            );
            vertex_map.insert((i * 2 + 1, j * 2 + 1), id);
        }
    }

    mesh
}

fn main() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                          T-SPLINE PERFORMANCE TEST                            ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    let simple_mesh = create_simple_tspline_mesh();
    let complex_mesh = create_complex_tspline_mesh();

    println!("📊 T-SPLINE BASIC OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Test basic operations
    benchmark("TSpline::add_vertex", || {
        let mut mesh = TSplineMesh::new();
        mesh.add_vertex(
            Point3::new(1.0, 2.0, 3.0),
            1.0,
            vec![0.0, 0.5, 1.0],
            vec![0.0, 0.5, 1.0],
        );
    });

    // Test evaluation
    let eval_ns = benchmark("TSpline::evaluate_point (simple)", || {
        let _ = simple_mesh.evaluate(0.5, 0.5);
    });

    benchmark("TSpline::evaluate_point (complex)", || {
        let _ = complex_mesh.evaluate(0.5, 0.5);
    });

    // Note: compute_basis_functions is private, skipping this test

    println!("\n📊 T-SPLINE ADVANCED OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Note: local_refinement method not exposed in current API

    // Note: insert_knot method not exposed in current API

    // Test conversion
    benchmark("TSpline::to_nurbs_patches", || {
        let _ = simple_mesh.to_nurbs();
    });

    println!("\n📊 T-SPLINE BATCH OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Test batch evaluation
    let params: Vec<(f64, f64)> = (0..10)
        .flat_map(|i| (0..10).map(move |j| (i as f64 / 9.0, j as f64 / 9.0)))
        .collect();

    let batch_simple_ns = benchmark("TSpline::evaluate (100 points, simple)", || {
        for &(u, v) in &params {
            let _ = simple_mesh.evaluate(u, v);
        }
    });

    let batch_complex_ns = benchmark("TSpline::evaluate (100 points, complex)", || {
        for &(u, v) in &params {
            let _ = complex_mesh.evaluate(u, v);
        }
    });

    println!("\n📈 PERFORMANCE SUMMARY");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Single evaluation (simple):  {:.1} ns/op", eval_ns);
    println!(
        "  Batch eval (simple):         {:.1} ns/op per point",
        batch_simple_ns / 100.0
    );
    println!(
        "  Batch eval (complex):        {:.1} ns/op per point",
        batch_complex_ns / 100.0
    );
    println!("  Industry target:             < 500 ns/op for T-splines");

    if eval_ns < 500.0 {
        println!("  ✅ T-spline evaluation: MEETS TARGET!");
    } else {
        println!("  ❌ T-spline evaluation: NEEDS OPTIMIZATION");
    }

    println!("\n📊 T-SPLINE MESH STATISTICS");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!(
        "  Simple mesh:   {} vertices, {} edges, {} faces",
        simple_mesh.vertices.len(),
        simple_mesh.edges.len(),
        simple_mesh.faces.len()
    );
    println!(
        "  Complex mesh:  {} vertices, {} edges, {} faces",
        complex_mesh.vertices.len(),
        complex_mesh.edges.len(),
        complex_mesh.faces.len()
    );

    // Test extraordinary vertices
    let extraordinary_count = simple_mesh
        .vertices
        .values()
        .filter(|v| v.is_extraordinary)
        .count();
    println!("  Extraordinary vertices: {}", extraordinary_count);
}
