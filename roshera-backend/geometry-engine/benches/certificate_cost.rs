//! Criterion measurement of the soundness-certificate compute cost — the
//! number the pitch deck carries as "~44 ms, memoized, sync under write lock".
//!
//! `BRepModel::certify_solid` is the kernel's self-certifying verdict. Its cost
//! has two regimes:
//!
//! * COLD — the per-solid cache is empty (`cached_certificate() == None`), so
//!   the call runs `compute_certificate`: `validate_solid_scoped` + a manifold /
//!   watertight / Euler pass over the tessellated mesh + a coarse
//!   self-intersection scan + tessellation-quality + mesh-quality + the dual-eye
//!   render-free reconcile. This is the price paid once per mutating op.
//! * MEMOIZED — the cache is warm; the call returns the stored certificate and
//!   only re-stamps the model-level `model_debris_orphan_faces` count (an
//!   O(faces) orphan scan that is intentionally never cached because it depends
//!   on OTHER solids in the model).
//!
//! Construction and boolean ops do NOT warm the cert cache (only `offset` does),
//! so a freshly built model gives a genuine cold measurement; `iter_batched`
//! rebuilds it per iteration. The memoized bench warms the cache once in setup
//! and times repeat reads.
//!
//! Three representative solids: a plain box, a boolean result (the f7 straddling
//! bore — union + difference + curved rim clipping, the heaviest classification
//! mix in the fleet), and an all-edges filleted cube.
//!
//! Release profile only carries the timing claim: run with
//! `CARGO_PROFILE_DEV_DEBUG=false CARGO_PROFILE_TEST_DEBUG=false \
//!  cargo bench -p geometry-engine --bench certificate_cost`.

// Reason for the allows — bench-only file: failing loudly at the setup site is
// the desired failure mode; the workspace deny lints target production code.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::fillet::{fillet_edges, FilletOptions, FilletType};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::edge::EdgeId;
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use std::time::Duration;

fn box_solid(m: &mut BRepModel, w: f64, h: f64, d: f64) -> SolidId {
    match TopologyBuilder::new(m).create_box_3d(w, h, d).expect("box") {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = box_solid(m, w, h, d);
    if tx != 0.0 {
        translate(m, vec![s], Vector3::X, tx, TransformOptions::default()).expect("tx");
    }
    if ty != 0.0 {
        translate(m, vec![s], Vector3::Y, ty, TransformOptions::default()).expect("ty");
    }
    if tz != 0.0 {
        translate(m, vec![s], Vector3::Z, tz, TransformOptions::default()).expect("tz");
    }
    s
}

fn cylinder(m: &mut BRepModel, base: Point3, axis: Vector3, radius: f64, height: f64) -> SolidId {
    match TopologyBuilder::new(m)
        .create_cylinder_3d(base, axis, radius, height)
        .expect("cylinder")
    {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    }
}

fn run_bool(m: &mut BRepModel, a: SolidId, b: SolidId, op: BooleanOp) -> SolidId {
    boolean_operation(m, a, b, op, BooleanOptions::default()).expect("boolean must complete")
}

fn all_edges(model: &BRepModel, solid: SolidId) -> Vec<EdgeId> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let Some(s) = model.solids.get(solid) else {
        return out;
    };
    let mut shells = vec![s.outer_shell];
    shells.extend_from_slice(&s.inner_shells);
    for sh in shells {
        let Some(shell) = model.shells.get(sh) else {
            continue;
        };
        for &fid in &shell.faces {
            let Some(face) = model.faces.get(fid) else {
                continue;
            };
            for lid in face.all_loops() {
                if let Some(lp) = model.loops.get(lid) {
                    for &e in &lp.edges {
                        if seen.insert(e) {
                            out.push(e);
                        }
                    }
                }
            }
        }
    }
    out
}

/// Plain 40×30×20 box.
fn build_box() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let s = box_solid(&mut m, 40.0, 30.0, 20.0);
    (m, s)
}

/// The f7 straddling bore: a 60×60×10 plate unioned with an r15 boss, then a
/// through r8 bore subtracted off-centre — union + difference + curved rim.
fn build_boolean() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let plate = box_at(&mut m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0);
    let boss = cylinder(&mut m, Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0);
    let u = run_bool(&mut m, plate, boss, BooleanOp::Union);
    let bore = cylinder(&mut m, Point3::new(10.0, 0.0, -3.0), Vector3::Z, 8.0, 26.0);
    let r = run_bool(&mut m, u, bore, BooleanOp::Difference);
    (m, r)
}

/// A 40 mm cube with every synthesizable edge rounded at r3 (graceful skip).
fn build_filleted() -> (BRepModel, SolidId) {
    let mut m = BRepModel::new();
    let cube = box_solid(&mut m, 40.0, 40.0, 40.0);
    let edges = all_edges(&m, cube);
    let opts = FilletOptions {
        fillet_type: FilletType::Constant(3.0),
        radius: 3.0,
        graceful_corner_skip: true,
        ..Default::default()
    };
    fillet_edges(&mut m, cube, edges, opts).expect("fillet must round something");
    (m, cube)
}

fn bench_certificate_cost(c: &mut Criterion) {
    let fixtures: &[(&str, fn() -> (BRepModel, SolidId))] = &[
        ("box", build_box),
        ("boolean_f7_bore", build_boolean),
        ("filleted_cube", build_filleted),
    ];

    // COLD: fresh model per iteration (untimed setup), one certify on an empty
    // cache. `certify_solid` returns owned `ValidityCertificate`, black-boxed.
    let mut cold = c.benchmark_group("certificate_cold");
    cold.sample_size(20);
    cold.warm_up_time(Duration::from_millis(500));
    cold.measurement_time(Duration::from_secs(12));
    for (name, build) in fixtures {
        cold.bench_function(*name, |b| {
            b.iter_batched(
                || build(),
                |(mut m, id)| black_box(m.certify_solid(black_box(id))),
                BatchSize::PerIteration,
            );
        });
    }
    cold.finish();

    // MEMOIZED: warm the cache once, then time repeat reads (cache hit + the
    // O(faces) model-debris re-stamp).
    let mut warm = c.benchmark_group("certificate_memoized");
    warm.sample_size(100);
    warm.warm_up_time(Duration::from_millis(500));
    warm.measurement_time(Duration::from_secs(6));
    for (name, build) in fixtures {
        warm.bench_function(*name, |b| {
            let (mut m, id) = build();
            let _warm = m.certify_solid(id); // populate cache
            b.iter(|| black_box(m.certify_solid(black_box(id))));
        });
    }
    warm.finish();
}

criterion_group!(benches, bench_certificate_cost);
criterion_main!(benches);
