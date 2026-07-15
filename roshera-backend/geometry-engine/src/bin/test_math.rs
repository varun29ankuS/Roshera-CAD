// Reason: benchmark/diagnostic harness binary -- fixtures are compile-time-
// constant; abort-on-failure is the harness's failure mode. The workspace
// production deny stands for library code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

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
