// Reason: demo binary -- aborting on a failed kernel call IS the demo's
// failure mode; the workspace production deny stands for library code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

//! Quick one-shot tessellation timing harness.
//!
//! Direct `Instant::now()` timing without Criterion's full pipeline —
//! used when we just need ballpark numbers and don't want to wait for
//! Criterion warm-up + sampling. Builds five primitives, tessellates
//! each at coarse/default/fine, prints wall-clock per call plus
//! triangle counts.
//!
//! Run with: `cargo run --release -p geometry-engine --example tess_quick`.

use std::time::Instant;

use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

fn build<F>(build_fn: F) -> Option<(BRepModel, SolidId)>
where
    F: FnOnce(&mut TopologyBuilder),
{
    let mut model = BRepModel::new();
    {
        let mut builder = TopologyBuilder::new(&mut model);
        build_fn(&mut builder);
    }
    let last_solid = model.solids.iter().last().map(|(id, _)| id)?;
    Some((model, last_solid))
}

fn time_shape(label: &str, model: &BRepModel, sid: SolidId, iters: u32) {
    let solid = match model.solids.get(sid) {
        Some(s) => s,
        None => {
            println!("  {label}: <build failed>");
            return;
        }
    };

    let presets: [(&str, TessellationParams); 3] = [
        ("coarse ", TessellationParams::coarse()),
        ("default", TessellationParams::default()),
        ("fine   ", TessellationParams::fine()),
    ];

    println!("== {label} ==");
    for (quality, params) in presets.iter() {
        // Warm-up.
        let mesh = tessellate_solid(solid, model, params);
        let tri_count = mesh.triangles.len();
        let vtx_count = mesh.vertices.len();

        let start = Instant::now();
        for _ in 0..iters {
            let _ = tessellate_solid(solid, model, params);
        }
        let elapsed = start.elapsed();
        let per_call_us = elapsed.as_micros() as f64 / iters as f64;

        println!(
            "  {quality}  {per_call_us:>9.1} µs/call   triangles={tri_count:>6}  vertices={vtx_count:>6}"
        );
    }
}

fn main() {
    println!("Tessellation timing — {} iters per preset\n", 50);

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_box_3d(10.0, 10.0, 10.0);
    }) {
        time_shape("box (10x10x10)", &model, sid, 50);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 5.0);
    }) {
        time_shape("sphere (r=5)", &model, sid, 50);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            2.0,
            5.0,
        );
    }) {
        time_shape("cylinder (r=2,h=5)", &model, sid, 50);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_cone_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            2.0,
            0.0,
            5.0,
        );
    }) {
        time_shape("cone (r=2,h=5)", &model, sid, 50);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_torus_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            5.0,
            1.0,
        );
    }) {
        time_shape("torus (R=5,r=1)", &model, sid, 50);
    }
}
