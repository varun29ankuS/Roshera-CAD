// Example demonstrating NURBS functionality
use geometry_engine::math::{nurbs::NurbsCurve, Vector3};

fn main() {
    println!("NURBS Example - Creating and evaluating a NURBS curve");

    // Create control points for a simple NURBS curve
    let control_points = vec![
        Vector3::new(0.0, 0.0, 0.0),
        Vector3::new(1.0, 1.0, 0.0),
        Vector3::new(2.0, 0.0, 0.0),
        Vector3::new(3.0, 1.0, 0.0),
    ];

    // Weights (uniform = 1.0 for all points)
    let weights = vec![1.0, 1.0, 1.0, 1.0];

    // Knot vector for degree 3 curve with 4 control points
    // For a degree 3 curve with 4 control points, we need 8 knots
    let knots = vec![0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];

    // Create a NURBS curve
    match NurbsCurve::new(control_points, weights, knots, 3) {
        Ok(curve) => {
            // Evaluate the curve at several points
            println!("\nEvaluating curve at different parameters:");
            for i in 0..=10 {
                let t = i as f64 / 10.0;
                let point = curve.evaluate(t);
                println!("t = {:.1}: {:?}", t, point);
            }
        }
        Err(e) => println!("Error creating NURBS curve: {}", e),
    }

    println!("\nNote: To run actual benchmarks, use: cargo bench");
}
