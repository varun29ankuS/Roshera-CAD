//! Roshera kernel demo — transforms.
//!
//! Creates a 50 mm cube, applies translate / rotate / scale through the
//! `transform.rs` entry points, tessellates after each step, and asserts
//! that the tessellated bounding box is consistent with the transform.
//!
//! This exercises both the transform plumbing and the planar-face
//! tessellator simultaneously — a transformed cube must still triangulate.
//!
//! Run with `cargo run --release --example demo_transforms`.

#[path = "common/mod.rs"]
mod common;

use std::f64::consts::FRAC_PI_2;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{rotate, scale, translate, TransformOptions};
use geometry_engine::primitives::box_primitive::{BoxParameters, BoxPrimitive};
use geometry_engine::primitives::builder::BRepModel;
use geometry_engine::primitives::primitive_traits::Primitive;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

use common::{header, model_summary, tess_and_write};

const SUBDIR: &str = "transforms";

#[derive(Debug, Clone, Copy)]
struct Bbox {
    min: [f64; 3],
    max: [f64; 3],
}

impl Bbox {
    fn dims(&self) -> [f64; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }
}

fn main() {
    header("transforms — translate, rotate, scale");

    let params = TessellationParams::default();

    // Identity → reference bbox.
    let mut model = BRepModel::new();
    let id = BoxPrimitive::create(
        BoxParameters::new(50.0, 50.0, 50.0).expect("box params"),
        &mut model,
    )
    .expect("box create");

    let identity_bbox = bbox_of(&model, id, &params);
    println!("[0] identity bbox: min={:?} max={:?}", identity_bbox.min, identity_bbox.max);
    let stats0 = tess_and_write(&model, id, &params, SUBDIR, "identity.stl");
    assert!(stats0.tris > 0, "identity box produced 0 triangles — planar-face defect");

    // Translate by (100, 50, 25): bbox shifts by the same amount; dims unchanged.
    let dir = Vector3::new(100.0, 50.0, 25.0);
    let dist = dir.magnitude();
    let unit = dir.normalize().expect("non-zero translation");
    translate(
        &mut model,
        vec![id],
        unit,
        dist,
        TransformOptions::default(),
    )
    .expect("translate");
    let translated_bbox = bbox_of(&model, id, &params);
    println!(
        "[1] translated bbox: min={:?} max={:?}",
        translated_bbox.min, translated_bbox.max
    );
    let stats1 = tess_and_write(&model, id, &params, SUBDIR, "translated.stl");
    assert!(stats1.tris > 0, "translated box produced 0 triangles");
    expect_close(translated_bbox.min[0] - identity_bbox.min[0], 100.0, "translate x");
    expect_close(translated_bbox.min[1] - identity_bbox.min[1],  50.0, "translate y");
    expect_close(translated_bbox.min[2] - identity_bbox.min[2],  25.0, "translate z");
    expect_close(translated_bbox.dims()[0], identity_bbox.dims()[0], "dims x preserved");
    expect_close(translated_bbox.dims()[1], identity_bbox.dims()[1], "dims y preserved");
    expect_close(translated_bbox.dims()[2], identity_bbox.dims()[2], "dims z preserved");

    // Rotate 90° about world Z (origin). For a 50³ box already at (100, 50, 25),
    // we just confirm tessellation still works and the dims permute appropriately.
    rotate(
        &mut model,
        vec![id],
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        FRAC_PI_2,
        TransformOptions::default(),
    )
    .expect("rotate");
    let rotated_bbox = bbox_of(&model, id, &params);
    println!(
        "[2] rotated   bbox: min={:?} max={:?}",
        rotated_bbox.min, rotated_bbox.max
    );
    let stats2 = tess_and_write(&model, id, &params, SUBDIR, "rotated.stl");
    assert!(stats2.tris > 0, "rotated box produced 0 triangles");
    // 90° about Z swaps x/y extents (50 mm cube remains a 50 mm cube).
    expect_close(rotated_bbox.dims()[0], identity_bbox.dims()[1], "rotated dim x = orig dim y");
    expect_close(rotated_bbox.dims()[1], identity_bbox.dims()[0], "rotated dim y = orig dim x");
    expect_close(rotated_bbox.dims()[2], identity_bbox.dims()[2], "rotated dim z preserved");

    // Scale 2× about origin: dims double.
    scale(
        &mut model,
        vec![id],
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(2.0, 2.0, 2.0),
        TransformOptions::default(),
    )
    .expect("scale");
    let scaled_bbox = bbox_of(&model, id, &params);
    println!(
        "[3] scaled    bbox: min={:?} max={:?}",
        scaled_bbox.min, scaled_bbox.max
    );
    let stats3 = tess_and_write(&model, id, &params, SUBDIR, "scaled.stl");
    assert!(stats3.tris > 0, "scaled box produced 0 triangles");
    expect_close(scaled_bbox.dims()[0], 2.0 * rotated_bbox.dims()[0], "scaled dim x");
    expect_close(scaled_bbox.dims()[1], 2.0 * rotated_bbox.dims()[1], "scaled dim y");
    expect_close(scaled_bbox.dims()[2], 2.0 * rotated_bbox.dims()[2], "scaled dim z");

    model_summary(&model);
    println!("\nAll transform invariants satisfied.");
}

fn bbox_of(model: &BRepModel, id: SolidId, params: &TessellationParams) -> Bbox {
    let solid = model.solids.get(id).expect("solid exists");
    let mesh = tessellate_solid(solid, model, params);
    let mut min = [f64::INFINITY; 3];
    let mut max = [f64::NEG_INFINITY; 3];
    for v in &mesh.vertices {
        for (i, coord) in [v.position.x, v.position.y, v.position.z].iter().enumerate() {
            if *coord < min[i] {
                min[i] = *coord;
            }
            if *coord > max[i] {
                max[i] = *coord;
            }
        }
    }
    Bbox { min, max }
}

fn expect_close(actual: f64, expected: f64, label: &str) {
    let tol = 1e-3;
    assert!(
        (actual - expected).abs() < tol,
        "{label}: expected {expected}, got {actual} (delta {})",
        (actual - expected).abs()
    );
}
