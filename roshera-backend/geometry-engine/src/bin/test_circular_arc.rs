// Test circular arc creation
use geometry_engine::math::nurbs::NurbsCurve;
use geometry_engine::math::{consts, Point3, Vector3};

fn main() {
    println!("Testing circular arc creation");

    // Test case 1: Arc in XY plane (normal = Z)
    let center = Point3::new(0.0, 0.0, 0.0);
    let radius = 1.0;
    let start_angle = 0.0;
    let sweep_angle = consts::PI / 2.0; // 90 degrees
    let normal = Vector3::Z;

    match NurbsCurve::circular_arc(center, radius, start_angle, sweep_angle, normal) {
        Ok(arc) => {
            println!("✅ Circular arc created successfully");

            // Test points
            let p0 = arc.evaluate(0.0).point;
            let p1 = arc.evaluate(1.0).point;

            println!("  Start point (u=0): {:?}", p0);
            println!("  End point (u=1): {:?}", p1);
            println!("  Expected start: (1, 0, 0)");
            println!("  Expected end: (0, 1, 0)");

            let start_error = (p0 - Point3::new(1.0, 0.0, 0.0)).magnitude();
            let end_error = (p1 - Point3::new(0.0, 1.0, 0.0)).magnitude();

            println!("  Start error: {}", start_error);
            println!("  End error: {}", end_error);

            if start_error < 1e-6 && end_error < 1e-6 {
                println!("✅ Circular arc test passed");
            } else {
                println!("❌ Circular arc test failed");
            }
        }
        Err(e) => {
            println!("❌ Failed to create circular arc: {}", e);
        }
    }
}
