//! Roshera kernel demo — extrude and revolve.
//!
//! Builds two profiles from scratch (a rectangle and an L-shape), extrudes
//! them along +Z, then revolves an off-axis rectangle around Z to make a
//! hollow ring. Tessellates each result and asserts a non-empty mesh.
//!
//! Run with `cargo run --release --example demo_extrude_revolve`.

#[path = "common/mod.rs"]
mod common;

use std::f64::consts::TAU;
use std::time::Instant;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::{
    extrude_profile, revolve_profile, ExtrudeOptions, RevolveOptions,
};
use geometry_engine::primitives::builder::BRepModel;
use geometry_engine::tessellation::TessellationParams;

use common::{add_line_edge, header, make_rectangle_profile, model_summary, tess_and_write};

const SUBDIR: &str = "extrude_revolve";

fn main() {
    header("extrude + revolve");

    let params = TessellationParams::default();

    extrude_rectangle(&params);
    extrude_l_shape(&params);
    revolve_rectangle(&params);

    println!("\nAll extrude/revolve outputs within acceptance bounds.");
}

/// 100×50 rectangle extruded 50 mm in +Z → simple rectangular block.
fn extrude_rectangle(params: &TessellationParams) {
    let mut model = BRepModel::new();
    let profile = make_rectangle_profile(&mut model, Point3::new(0.0, 0.0, 0.0), 100.0, 50.0);

    let t = Instant::now();
    let id = extrude_profile(
        &mut model,
        profile,
        ExtrudeOptions {
            direction: Vector3::new(0.0, 0.0, 1.0),
            distance: 50.0,
            cap_ends: true,
            ..Default::default()
        },
    )
    .expect("extrude rectangle");
    println!(
        "[1] extrude rectangle 100x50 by 50 -> solid #{id}  ({:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );

    let stats = tess_and_write(&model, id, params, SUBDIR, "rect_extrude.stl");
    assert!(stats.tris > 0, "extruded rectangle produced 0 triangles");
    model_summary(&model);
}

/// L-shaped profile (6 vertices, 6 edges) extruded by 30 mm.
fn extrude_l_shape(params: &TessellationParams) {
    let mut model = BRepModel::new();

    // L-shape outline (CCW, all in XY at z=0):
    //   (0,0) -> (60,0) -> (60,20) -> (20,20) -> (20,40) -> (0,40) -> (0,0)
    let v0 = model.vertices.add(0.0, 0.0, 0.0);
    let v1 = model.vertices.add(60.0, 0.0, 0.0);
    let v2 = model.vertices.add(60.0, 20.0, 0.0);
    let v3 = model.vertices.add(20.0, 20.0, 0.0);
    let v4 = model.vertices.add(20.0, 40.0, 0.0);
    let v5 = model.vertices.add(0.0, 40.0, 0.0);
    let profile = vec![
        add_line_edge(&mut model, v0, v1),
        add_line_edge(&mut model, v1, v2),
        add_line_edge(&mut model, v2, v3),
        add_line_edge(&mut model, v3, v4),
        add_line_edge(&mut model, v4, v5),
        add_line_edge(&mut model, v5, v0),
    ];

    let t = Instant::now();
    let id = extrude_profile(
        &mut model,
        profile,
        ExtrudeOptions {
            direction: Vector3::new(0.0, 0.0, 1.0),
            distance: 30.0,
            cap_ends: true,
            ..Default::default()
        },
    )
    .expect("extrude L-shape");
    println!(
        "[2] extrude L-shape         by 30 -> solid #{id}  ({:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );

    let stats = tess_and_write(&model, id, params, SUBDIR, "l_extrude.stl");
    assert!(stats.tris > 0, "extruded L-shape produced 0 triangles");
    model_summary(&model);
}

/// Rectangle in the XZ plane offset from Z axis, revolved 360° about Z → ring.
fn revolve_rectangle(params: &TessellationParams) {
    let mut model = BRepModel::new();

    // Profile lives in XZ plane, x ∈ [20, 30], z ∈ [0, 10] (at y = 0).
    let v0 = model.vertices.add(20.0, 0.0, 0.0);
    let v1 = model.vertices.add(30.0, 0.0, 0.0);
    let v2 = model.vertices.add(30.0, 0.0, 10.0);
    let v3 = model.vertices.add(20.0, 0.0, 10.0);
    let profile = vec![
        add_line_edge(&mut model, v0, v1),
        add_line_edge(&mut model, v1, v2),
        add_line_edge(&mut model, v2, v3),
        add_line_edge(&mut model, v3, v0),
    ];

    let t = Instant::now();
    let id = revolve_profile(
        &mut model,
        profile,
        RevolveOptions {
            axis_origin: Point3::new(0.0, 0.0, 0.0),
            axis_direction: Vector3::new(0.0, 0.0, 1.0),
            angle: TAU,
            segments: 32,
            cap_ends: false,
            ..Default::default()
        },
    )
    .expect("revolve rectangle");
    println!(
        "[3] revolve rect (R=20-30, h=10) -> solid #{id}  ({:.2} ms)",
        t.elapsed().as_secs_f64() * 1e3
    );

    let stats = tess_and_write(&model, id, params, SUBDIR, "ring_revolve.stl");
    assert!(stats.tris > 0, "revolved ring produced 0 triangles");
    model_summary(&model);
}
