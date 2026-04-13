// Simple test to verify math module compiles with cargo
use geometry_engine::math::{Matrix4, Point3, Vector3};

fn main() {
    let v = Vector3::new(1.0, 2.0, 3.0);
    let p = Point3::new(0.0, 0.0, 0.0);
    let m = Matrix4::identity();

    println!("Math module test:");
    println!("Vector: {:?}", v);
    println!("Point: {:?}", p);
    println!("Matrix: {:?}", m);
}
