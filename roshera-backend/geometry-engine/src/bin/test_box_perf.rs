use geometry_engine::primitives::{
    box_primitive::{BoxParameters, BoxPrimitive},
    primitive_traits::Primitive,
    topology_builder::BRepModel,
};
use std::time::Instant;

fn main() {
    println!("Testing Box primitive performance...");

    // Warm up
    for _ in 0..10 {
        let mut model = BRepModel::new();
        let params = BoxParameters::new(10.0, 10.0, 10.0).unwrap();
        let _ = BoxPrimitive::create(params, &mut model);
    }

    // Actual timing
    let iterations = 100;
    let start = Instant::now();

    for _ in 0..iterations {
        let mut model = BRepModel::new();
        let params = BoxParameters::new(10.0, 10.0, 10.0).unwrap();
        let _ = BoxPrimitive::create(params, &mut model).unwrap();
    }

    let duration = start.elapsed();
    let avg_time = duration / iterations;

    println!("Average Box creation time: {:?}", avg_time);
    println!("Target: <100μs");

    if avg_time.as_micros() < 100 {
        println!("✅ PASS - Performance target met!");
    } else {
        println!("❌ FAIL - Performance target not met!");
    }
}
