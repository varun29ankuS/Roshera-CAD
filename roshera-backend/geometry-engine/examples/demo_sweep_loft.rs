//! Roshera kernel demo — sweep and loft.
//!
//! Sweeps a circular profile along a straight path, and lofts between three
//! parallel profiles (circle → square → circle). Tessellates the resulting
//! solids and asserts non-empty meshes.
//!
//! Run with `cargo run --release --example demo_sweep_loft`.

#[path = "common/mod.rs"]
mod common;

use std::time::Instant;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{loft_profiles, sweep_profile, LoftOptions, SweepOptions};
use geometry_engine::primitives::builder::BRepModel;
use geometry_engine::tessellation::TessellationParams;

use common::{
    add_line_edge, header, make_circle_profile, make_rectangle_profile, model_summary,
    tess_and_write,
};

const SUBDIR: &str = "sweep_loft";

fn main() {
    header("sweep + loft");

    let params = TessellationParams::default();

    sweep_circle_along_line(&params);
    loft_circle_square_circle(&params);

    println!("\nAll sweep/loft outputs within acceptance bounds.");
}

/// r=5 circle profile swept along a 100 mm line in +X.
fn sweep_circle_along_line(params: &TessellationParams) {
    let mut model = BRepModel::new();

    // Circular profile centered at origin, normal +X (so the profile lies
    // in the YZ plane and points along the path).
    let profile = make_circle_profile(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(1.0, 0.0, 0.0),
        5.0,
    );

    // Path: straight edge from origin to (100, 0, 0).
    let p_start = model.vertices.add(0.0, 0.0, 0.0);
    let p_end = model.vertices.add(100.0, 0.0, 0.0);
    let path = add_line_edge(&mut model, p_start, p_end);

    let t = Instant::now();
    let id = sweep_profile(
        &mut model,
        profile,
        path,
        SweepOptions {
            create_solid: true,
            ..Default::default()
        },
    )
    .expect("sweep circle along line");
    println!(
        "[1] sweep r=5 circle along x-axis 100 mm -> solid #{id}  ({:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );

    let stats = tess_and_write(&model, id, params, SUBDIR, "circle_sweep.stl");
    assert!(stats.tris > 0, "swept solid produced 0 triangles");
    model_summary(&model);
}

/// Loft three parallel profiles: circle at z=0, square at z=50, circle at z=100.
fn loft_circle_square_circle(params: &TessellationParams) {
    let mut model = BRepModel::new();

    let circle_bot = make_circle_profile(
        &mut model,
        Point3::new(0.0, 0.0, 0.0),
        Vector3::new(0.0, 0.0, 1.0),
        25.0,
    );

    let square_mid =
        make_rectangle_profile(&mut model, Point3::new(-20.0, -20.0, 50.0), 40.0, 40.0);

    let circle_top = make_circle_profile(
        &mut model,
        Point3::new(0.0, 0.0, 100.0),
        Vector3::new(0.0, 0.0, 1.0),
        15.0,
    );

    let t = Instant::now();
    let id = loft_profiles(
        &mut model,
        vec![circle_bot, square_mid, circle_top],
        LoftOptions {
            closed: false,
            create_solid: true,
            ..Default::default()
        },
    )
    .expect("loft circle-square-circle");
    println!(
        "[2] loft circle25 -> square40 -> circle15 -> solid #{id}  ({:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );

    let stats = tess_and_write(&model, id, params, SUBDIR, "tri_loft.stl");
    assert!(stats.tris > 0, "lofted solid produced 0 triangles");
    model_summary(&model);
}
