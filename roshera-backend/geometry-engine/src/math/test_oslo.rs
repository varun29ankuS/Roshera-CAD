// Simple test for Oslo knot insertion algorithm
use crate::math::nurbs::NurbsCurve;
use crate::math::Point3;

pub fn test_oslo_simple() {
    println!("Testing Oslo knot insertion algorithm");

    // Create a simple quadratic curve
    let control_points = vec![
        Point3::new(0.0, 0.0, 0.0),
        Point3::new(1.0, 1.0, 0.0),
        Point3::new(2.0, 0.0, 0.0),
    ];

    let weights = vec![1.0, 1.0, 1.0];
    let knots = vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0];
    let degree = 2;

    let mut curve = NurbsCurve::new(control_points, weights, knots, degree).unwrap();

    println!("Original curve:");
    println!("  Control points: {:?}", curve.control_points);
    println!("  Weights: {:?}", curve.weights);
    println!("  Knots: {:?}", curve.knots.values());

    // Evaluate before insertion
    let p_before = curve.evaluate(0.5).point;
    println!("  Point at u=0.5 before: {:?}", p_before);

    // Insert knot at u=0.5
    match curve.insert_knot(0.5, 1) {
        Ok(_) => {
            println!("Knot insertion successful");

            println!("After knot insertion:");
            println!("  Control points: {:?}", curve.control_points);
            println!("  Weights: {:?}", curve.weights);
            println!("  Knots: {:?}", curve.knots.values());

            // Evaluate after insertion
            let p_after = curve.evaluate(0.5).point;
            println!("  Point at u=0.5 after: {:?}", p_after);

            let diff = (p_before - p_after).magnitude();
            println!("  Difference magnitude: {}", diff);

            if diff < 1e-6 {
                println!("✅ Test passed - curve shape preserved");
            } else {
                println!("❌ Test failed - curve shape changed by {}", diff);
            }
        }
        Err(e) => {
            println!("❌ Knot insertion failed: {}", e);
        }
    }
}
