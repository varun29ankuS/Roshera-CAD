//! Component-level performance tests to isolate bottlenecks
//!
//! Tests each component (vertices, curves, edges, etc.) individually
//! to identify which shared component is causing the performance degradation.

use crate::math::{Point3, Vector3};
use crate::primitives::{
    curve::{Line, ParameterRange},
    edge::{Edge, EdgeOrientation},
    face::{Face, FaceOrientation},
    r#loop::{Loop, LoopType},
    shell::{Shell, ShellType},
    solid::Solid,
    surface::Plane,
    topology_builder::BRepModel,
};
use std::time::Instant;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vertex_performance() {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║                    VERTEX PERFORMANCE TEST                        ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");

        let iterations = 1000;
        let mut vertex_times = Vec::new();

        for _ in 0..10 {
            let mut model = BRepModel::new();
            let start = Instant::now();

            for i in 0..iterations {
                model.vertices.add_or_find(i as f64, 0.0, 0.0, 0.001);
            }

            vertex_times.push(start.elapsed());
        }

        let avg_vertex_time =
            vertex_times.iter().sum::<std::time::Duration>() / vertex_times.len() as u32;
        let per_vertex = avg_vertex_time / iterations as u32;

        println!("  📊 VERTEX PERFORMANCE RESULTS:");
        println!(
            "    Total time for {} vertices: {:?}",
            iterations, avg_vertex_time
        );
        println!("    Per vertex: {:?}", per_vertex);
        println!("    Target per vertex: <1μs");

        if per_vertex.as_nanos() > 1000 {
            println!("    ❌ VERTEX CREATION TOO SLOW");
        } else {
            println!("    ✅ Vertex creation within target");
        }
    }

    #[test]
    fn test_curve_performance() {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║                    CURVE PERFORMANCE TEST                         ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");

        let iterations = 1000;
        let mut curve_times = Vec::new();

        for _ in 0..10 {
            let mut model = BRepModel::new();
            let start = Instant::now();

            for i in 0..iterations {
                let line = Line::new(
                    Point3::new(i as f64, 0.0, 0.0),
                    Point3::new(i as f64 + 1.0, 0.0, 0.0),
                );
                model.curves.add(Box::new(line));
            }

            curve_times.push(start.elapsed());
        }

        let avg_curve_time =
            curve_times.iter().sum::<std::time::Duration>() / curve_times.len() as u32;
        let per_curve = avg_curve_time / iterations as u32;

        println!("  📊 CURVE PERFORMANCE RESULTS:");
        println!(
            "    Total time for {} curves: {:?}",
            iterations, avg_curve_time
        );
        println!("    Per curve: {:?}", per_curve);
        println!("    Target per curve: <500ns");

        if per_curve.as_nanos() > 500 {
            println!("    ❌ CURVE CREATION TOO SLOW");
        } else {
            println!("    ✅ Curve creation within target");
        }
    }

    #[test]
    fn test_edge_performance() {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║                    EDGE PERFORMANCE TEST                          ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");

        let iterations = 1000;
        let mut edge_times = Vec::new();

        for _ in 0..10 {
            let mut model = BRepModel::new();

            // Pre-create vertices and curves
            let mut vertices = Vec::new();
            let mut curves = Vec::new();

            for i in 0..iterations {
                vertices.push(model.vertices.add_or_find(i as f64, 0.0, 0.0, 0.001));
                vertices.push(model.vertices.add_or_find(i as f64 + 1.0, 0.0, 0.0, 0.001));

                let line = Line::new(
                    Point3::new(i as f64, 0.0, 0.0),
                    Point3::new(i as f64 + 1.0, 0.0, 0.0),
                );
                curves.push(model.curves.add(Box::new(line)));
            }

            let start = Instant::now();

            for i in 0..iterations {
                let edge = Edge::new(
                    0,
                    vertices[i * 2],
                    vertices[i * 2 + 1],
                    curves[i],
                    EdgeOrientation::Forward,
                    ParameterRange::unit(),
                );
                model.edges.add_or_find(edge);
            }

            edge_times.push(start.elapsed());
        }

        let avg_edge_time =
            edge_times.iter().sum::<std::time::Duration>() / edge_times.len() as u32;
        let per_edge = avg_edge_time / iterations as u32;

        println!("  📊 EDGE PERFORMANCE RESULTS:");
        println!(
            "    Total time for {} edges: {:?}",
            iterations, avg_edge_time
        );
        println!("    Per edge: {:?}", per_edge);
        println!("    Target per edge: <200ns");

        if per_edge.as_nanos() > 200 {
            println!("    ❌ EDGE CREATION TOO SLOW");
        } else {
            println!("    ✅ Edge creation within target");
        }
    }

    #[test]
    fn test_surface_performance() {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║                   SURFACE PERFORMANCE TEST                        ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");

        let iterations = 1000;
        let mut surface_times = Vec::new();

        for _ in 0..10 {
            let mut model = BRepModel::new();
            let start = Instant::now();

            for i in 0..iterations {
                let plane =
                    Plane::from_point_normal(Point3::new(i as f64, 0.0, 0.0), Vector3::Z).unwrap();
                model.surfaces.add(Box::new(plane));
            }

            surface_times.push(start.elapsed());
        }

        let avg_surface_time =
            surface_times.iter().sum::<std::time::Duration>() / surface_times.len() as u32;
        let per_surface = avg_surface_time / iterations as u32;

        println!("  📊 SURFACE PERFORMANCE RESULTS:");
        println!(
            "    Total time for {} surfaces: {:?}",
            iterations, avg_surface_time
        );
        println!("    Per surface: {:?}", per_surface);
        println!("    Target per surface: <1μs");

        if per_surface.as_nanos() > 1000 {
            println!("    ❌ SURFACE CREATION TOO SLOW");
        } else {
            println!("    ✅ Surface creation within target");
        }
    }

    #[test]
    fn test_face_performance() {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║                    FACE PERFORMANCE TEST                          ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");

        let iterations = 1000;
        let mut face_times = Vec::new();

        for _ in 0..10 {
            let mut model = BRepModel::new();

            // Pre-create surfaces and loops
            let mut surfaces = Vec::new();
            let mut loops = Vec::new();

            for i in 0..iterations {
                let plane =
                    Plane::from_point_normal(Point3::new(i as f64, 0.0, 0.0), Vector3::Z).unwrap();
                surfaces.push(model.surfaces.add(Box::new(plane)));

                let loop_ = Loop::new(0, LoopType::Outer);
                loops.push(model.loops.add(loop_));
            }

            let start = Instant::now();

            for i in 0..iterations {
                let face = Face::new(0, surfaces[i], loops[i], FaceOrientation::Forward);
                model.faces.add(face);
            }

            face_times.push(start.elapsed());
        }

        let avg_face_time =
            face_times.iter().sum::<std::time::Duration>() / face_times.len() as u32;
        let per_face = avg_face_time / iterations as u32;

        println!("  📊 FACE PERFORMANCE RESULTS:");
        println!(
            "    Total time for {} faces: {:?}",
            iterations, avg_face_time
        );
        println!("    Per face: {:?}", per_face);
        println!("    Target per face: <50ns");

        if per_face.as_nanos() > 50 {
            println!("    ❌ FACE CREATION TOO SLOW");
        } else {
            println!("    ✅ Face creation within target");
        }
    }

    #[test]
    fn test_component_performance_summary() {
        println!("\n╔══════════════════════════════════════════════════════════════════╗");
        println!("║              COMPONENT PERFORMANCE SUMMARY                        ║");
        println!("╚══════════════════════════════════════════════════════════════════╝");

        // Run a quick test of all components
        let mut model = BRepModel::new();
        let iterations = 100;

        // Vertex test
        let start = Instant::now();
        for i in 0..iterations {
            model.vertices.add_or_find(i as f64, 0.0, 0.0, 0.001);
        }
        let vertex_time = start.elapsed() / iterations as u32;

        // Curve test
        let start = Instant::now();
        for i in 0..iterations {
            let line = Line::new(
                Point3::new(i as f64, 0.0, 0.0),
                Point3::new(i as f64 + 1.0, 0.0, 0.0),
            );
            model.curves.add(Box::new(line));
        }
        let curve_time = start.elapsed() / iterations as u32;

        // Surface test
        let start = Instant::now();
        for i in 0..iterations {
            let plane =
                Plane::from_point_normal(Point3::new(i as f64, 0.0, 0.0), Vector3::Z).unwrap();
            model.surfaces.add(Box::new(plane));
        }
        let surface_time = start.elapsed() / iterations as u32;

        println!("  📊 QUICK COMPONENT BENCHMARK:");
        println!("    Vertex creation:  {:?} (target: <1μs)", vertex_time);
        println!("    Curve creation:   {:?} (target: <500ns)", curve_time);
        println!("    Surface creation: {:?} (target: <1μs)", surface_time);

        // Identify the slowest component
        let slowest = if vertex_time > curve_time && vertex_time > surface_time {
            "VERTICES"
        } else if curve_time > surface_time {
            "CURVES"
        } else {
            "SURFACES"
        };

        println!("\n  🎯 SLOWEST COMPONENT: {}", slowest);
        println!("  ⚡ Focus optimization efforts on this component first!");
    }
}
