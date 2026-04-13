use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::{
    box_primitive::{BoxParameters, BoxPrimitive},
    builder::BRepModel,
    cylinder_primitive::{CylinderParameters, CylinderPrimitive},
    primitive_traits::Primitive,
    sphere_primitive::{SphereParameters, SpherePrimitive},
};

fn main() {
    println!("🧪 Testing Basic Primitive Creation");

    // Create a new B-Rep model
    let mut model = BRepModel::new();

    println!("📦 Testing box creation...");
    match BoxParameters::new(10.0, 5.0, 3.0) {
        Ok(box_params) => match BoxPrimitive::create(box_params, &mut model) {
            Ok(box_id) => {
                println!("✅ Box created successfully with ID: {:?}", box_id);
            }
            Err(e) => {
                println!("❌ Box creation failed: {:?}", e);
            }
        },
        Err(e) => {
            println!("❌ Box parameter creation failed: {:?}", e);
        }
    }

    println!("🔵 Testing sphere creation...");
    match SphereParameters::new(5.0, Point3::new(10.0, 0.0, 0.0)) {
        Ok(sphere_params) => match SpherePrimitive::create(sphere_params, &mut model) {
            Ok(sphere_id) => {
                println!("✅ Sphere created successfully with ID: {:?}", sphere_id);
            }
            Err(e) => {
                println!("❌ Sphere creation failed: {:?}", e);
            }
        },
        Err(e) => {
            println!("❌ Sphere parameter creation failed: {:?}", e);
        }
    }

    println!("🔧 Testing cylinder creation...");
    match CylinderParameters::new(3.0, 8.0) {
        Ok(cylinder_params) => match cylinder_params.with_axis(Vector3::new(0.0, 0.0, 1.0)) {
            Ok(final_params) => match CylinderPrimitive::create(final_params, &mut model) {
                Ok(cylinder_id) => {
                    println!(
                        "✅ Cylinder created successfully with ID: {:?}",
                        cylinder_id
                    );
                }
                Err(e) => {
                    println!("❌ Cylinder creation failed: {:?}", e);
                }
            },
            Err(e) => {
                println!("❌ Cylinder axis setting failed: {:?}", e);
            }
        },
        Err(e) => {
            println!("❌ Cylinder parameter creation failed: {:?}", e);
        }
    }

    println!("🧪 Primitive creation test completed!");
}
