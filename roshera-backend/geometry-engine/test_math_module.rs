// Quick test to verify math module compiles
use geometry_engine::math::{Vector3, Point3, Matrix4};

fn main() {
    let v = Vector3::new(1.0, 2.0, 3.0);
    let p = Point3::new(0.0, 0.0, 0.0);
    let m = Matrix4::identity();
    
    println!("Vector: {:?}", v);
    println!("Point: {:?}", p);
    println!("Matrix: {:?}", m);
}