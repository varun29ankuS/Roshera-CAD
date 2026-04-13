use crate::math::{tspline::TSplineMesh, Point3};
use std::time::Instant;

const WARMUP_ITERATIONS: usize = 100;
const BENCHMARK_ITERATIONS: usize = 1_000; // Much lower for T-splines

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
        } else if ops_per_sec >= 1_000.0 {
            format!("{:.1}K", ops_per_sec / 1_000.0)
        } else {
            format!("{:.1}", ops_per_sec)
        }
    );

    ns_per_op
}

fn create_simple_tspline_mesh() -> TSplineMesh {
    let mut mesh = TSplineMesh::new();

    // Create a simple 2x2 grid of vertices
    let mut vertex_ids = Vec::new();
    for i in 0..2 {
        for j in 0..2 {
            let id = mesh.add_vertex(
                Point3::new(i as f64, j as f64, 0.0),
                1.0,            // uniform weight
                vec![0.0, 1.0], // simple knot intervals
                vec![0.0, 1.0],
            );
            vertex_ids.push(id);
        }
    }

    // Create one face
    let _ = mesh.add_face(vec![
        vertex_ids[0],
        vertex_ids[1],
        vertex_ids[3],
        vertex_ids[2],
    ]);

    mesh
}

fn main() {
    println!("\n╔══════════════════════════════════════════════════════════════════════════════╗");
    println!("║                       SIMPLE T-SPLINE PERFORMANCE TEST                        ║");
    println!("╚══════════════════════════════════════════════════════════════════════════════╝\n");

    let mesh = create_simple_tspline_mesh();

    println!("📊 T-SPLINE OPERATIONS");
    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Test basic operations
    let add_vertex_ns = benchmark("TSpline::add_vertex", || {
        let mut mesh = TSplineMesh::new();
        mesh.add_vertex(
            Point3::new(1.0, 2.0, 3.0),
            1.0,
            vec![0.0, 1.0],
            vec![0.0, 1.0],
        );
    });

    // Test evaluation
    let eval_ns = benchmark("TSpline::evaluate (single point)", || {
        let _ = mesh.evaluate(0.5, 0.5);
    });

    // Test conversion
    let convert_ns = benchmark("TSpline::to_nurbs", || {
        let _ = mesh.to_nurbs();
    });

    // Test batch evaluation (smaller batch)
    let params: Vec<(f64, f64)> = (0..5)
        .flat_map(|i| (0..5).map(move |j| (i as f64 / 4.0, j as f64 / 4.0)))
        .collect();

    let batch_ns = benchmark("TSpline::evaluate (25 points batch)", || {
        for &(u, v) in &params {
            let _ = mesh.evaluate(u, v);
        }
    });

    println!("\n📈 PERFORMANCE SUMMARY");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Vertex creation:      {:.1} ns/op", add_vertex_ns);
    println!("  Single evaluation:    {:.1} ns/op", eval_ns);
    println!("  NURBS conversion:     {:.1} ns/op", convert_ns);
    println!(
        "  Batch eval (25 pts):  {:.1} ns/op per point",
        batch_ns / 25.0
    );
    println!("  Industry target:      < 500 ns/op for T-splines");

    if eval_ns < 500.0 {
        println!("  ✅ T-spline evaluation: MEETS TARGET!");
    } else if eval_ns < 1000.0 {
        println!(
            "  ⚠️  T-spline evaluation: Close to target ({}x slower)",
            (eval_ns / 500.0) as i32
        );
    } else {
        println!(
            "  ❌ T-spline evaluation: NEEDS OPTIMIZATION ({}x slower)",
            (eval_ns / 500.0) as i32
        );
    }

    println!("\n📊 MESH STATISTICS");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Vertices: {}", mesh.vertices.len());
    println!("  Faces: {}", mesh.faces.len());
    println!("  Edges: {}", mesh.edges.len());
}
