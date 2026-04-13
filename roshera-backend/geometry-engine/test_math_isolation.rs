// Minimal test to check if math module compiles
#![allow(dead_code)]

// Include the math module directly
#[path = "src/math/mod.rs"]
mod math;

fn main() {
    println!("Testing math module compilation in isolation");
    
    // Try to use basic math types
    let v = math::vector3::Vector3::new(1.0, 2.0, 3.0);
    println!("Vector3: {:?}", v);
}