//! Tessellation benchmark harness.
//!
//! Tracks per-primitive tessellation cost across the three quality
//! presets exposed on the REST surface (coarse / default / fine).
//! Each benchmark builds its primitive once outside the timed loop,
//! then re-runs `tessellate_solid` against the prebuilt BRep so we
//! measure tessellation work only — vertex/edge/face/surface store
//! construction is excluded.
//!
//! The shape set is the minimum that exercises every code path
//! touched by T-1..T-4:
//!
//! * **box** — six planar faces. Pure ear-clipping baseline; should
//!   not move when curvature-adaptive sampling changes.
//! * **sphere** — UV grid with two poles. Density grows with
//!   `chord_tolerance` (T-1) and `max_angle_deviation` (T-4 angle
//!   guard on the equator).
//! * **cylinder** — lateral face + two circular caps. Stresses the
//!   watertight invariant between `compute_curve_sample_count` and
//!   `arc_steps_for_quality` (T-4).
//! * **cone** — apex singularity handling.
//! * **torus** — two-axis curvature (major + minor), both subject
//!   to the triple-guard.
//!
//! Run with: `cargo bench --bench tessellation_bench`.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, TopologyBuilder};
use geometry_engine::tessellation::{tessellate_solid, TessellationParams};

/// Build a primitive into a fresh `BRepModel` and return the
/// `SolidId` of the last solid added. Returns `None` if construction
/// fails (e.g. degenerate parameters) so the benchmark is skipped
/// instead of panicking.
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

/// Time `tessellate_solid` on a prebuilt model at each quality preset.
fn bench_shape(c: &mut Criterion, label: &str, model: &BRepModel, sid: SolidId) {
    let solid = match model.solids.get(sid) {
        Some(s) => s,
        None => return,
    };

    let presets: [(&str, TessellationParams); 3] = [
        ("coarse", TessellationParams::coarse()),
        ("default", TessellationParams::default()),
        ("fine", TessellationParams::fine()),
    ];

    let mut group = c.benchmark_group(format!("tessellate_{label}"));
    for (quality, params) in presets.iter() {
        group.bench_with_input(
            BenchmarkId::from_parameter(quality),
            params,
            |b, params| {
                b.iter(|| {
                    let mesh = tessellate_solid(
                        black_box(solid),
                        black_box(model),
                        black_box(params),
                    );
                    black_box(mesh)
                });
            },
        );
    }
    group.finish();
}

fn bench_tessellation(c: &mut Criterion) {
    if let Some((model, sid)) = build(|b| {
        let _ = b.create_box_3d(10.0, 10.0, 10.0);
    }) {
        bench_shape(c, "box", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_sphere_3d(Point3::new(0.0, 0.0, 0.0), 5.0);
    }) {
        bench_shape(c, "sphere", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_cylinder_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            2.0,
            5.0,
        );
    }) {
        bench_shape(c, "cylinder", &model, sid);
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
        bench_shape(c, "cone", &model, sid);
    }

    if let Some((model, sid)) = build(|b| {
        let _ = b.create_torus_3d(
            Point3::new(0.0, 0.0, 0.0),
            Vector3::new(0.0, 0.0, 1.0),
            5.0,
            1.0,
        );
    }) {
        bench_shape(c, "torus", &model, sid);
    }
}

criterion_group!(benches, bench_tessellation);
criterion_main!(benches);
