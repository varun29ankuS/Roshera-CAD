//! Criterion baseline for classification-heavy boolean fixtures — EXACT
//! PREDICATES campaign Slice 1 (spec §3.7: "No boolean benchmark exists …
//! that absence is Slice 1's first RED").
//!
//! Fixtures are drawn from the existing regression fleet so the numbers mean
//! something to the campaign's later slices:
//!
//! - `union/box_box_overlap` — plain overlapping-box union (planar split +
//!   classification floor).
//! - `union/coincident_boss_cap_merge` — the f7 union half (60×60×10 plate ∪
//!   r15 boss with coincident-coplanar z=0 bottoms): drives the cap-merge /
//!   same-domain probes + planar PIP paths.
//! - `union/chain3_prisms` — three sequential box unions (#27 chained-union
//!   family): classification against progressively fragmented topology.
//! - `difference/box_minus_cylinder_poke` — through-poke curved cell (poke-
//!   matrix family): curved splitting + GWN/analytic membership.
//! - `difference/straddling_bore_f7_offset10` — the full f7 straddling-rim
//!   difference (setup builds the boss union untimed): the heaviest
//!   classification mix in the fleet (coplanar seams + curved rim clipping +
//!   hole nesting + selection).
//! - `intersection/box_cylinder` — intersection selection over the same
//!   curved split.
//!
//! Models are rebuilt in setup per iteration (`iter_batched`, untimed):
//! `BRepModel` is not `Clone`, and boolean ops mutate the model. Sample size
//! is small (10) — this is a regression budget baseline (spec §3.7: ≤5% per
//! fixture per migration slice), not a micro-optimisation harness. Baselines
//! are recorded in `.superpowers/sdd/exact-predicates-slice1-2-report.md`.
//!
//! The spec also names a 1k-face union (the CLAUDE.md <100 ms budget case).
//! No 1k-face operand builder exists in the fleet and `BRepModel` cannot be
//! cloned out of a one-time construction, so per-iteration setup would be
//! ~166 chained drill booleans — recorded in the Slice-1 ledger as a spec
//! delta; the budget case stays owned by the CLAUDE.md table until a
//! reusable big-operand fixture exists.

// Reason for the allows — bench-only file: failing loudly at the setup site
// is the desired failure mode; the workspace deny lints target production.
#![allow(clippy::expect_used)]
#![allow(clippy::unwrap_used)]
#![allow(clippy::panic)]

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use geometry_engine::math::{Point3, Vector3};
use geometry_engine::operations::boolean::{boolean_operation, BooleanOp, BooleanOptions};
use geometry_engine::operations::transform::{translate, TransformOptions};
use geometry_engine::primitives::solid::SolidId;
use geometry_engine::primitives::topology_builder::{BRepModel, GeometryId, TopologyBuilder};
use std::time::Duration;

fn box_at(m: &mut BRepModel, w: f64, h: f64, d: f64, tx: f64, ty: f64, tz: f64) -> SolidId {
    let s = match TopologyBuilder::new(m).create_box_3d(w, h, d).expect("box") {
        GeometryId::Solid(s) => s,
        o => panic!("expected solid, got {o:?}"),
    };
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

fn bench_boolean_classification(c: &mut Criterion) {
    let mut group = c.benchmark_group("boolean_classification");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(8));

    group.bench_function("union/box_box_overlap", |b| {
        b.iter_batched(
            || {
                let mut m = BRepModel::new();
                let a = box_at(&mut m, 20.0, 20.0, 10.0, 0.0, 0.0, 5.0);
                let bx = box_at(&mut m, 20.0, 20.0, 10.0, 8.0, 6.0, 11.0);
                (m, a, bx)
            },
            |(mut m, a, bx)| run_bool(&mut m, a, bx, BooleanOp::Union),
            BatchSize::PerIteration,
        );
    });

    group.bench_function("union/coincident_boss_cap_merge", |b| {
        b.iter_batched(
            || {
                let mut m = BRepModel::new();
                let plate = box_at(&mut m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0); // z∈[0,10]
                let boss = cylinder(&mut m, Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0);
                (m, plate, boss)
            },
            |(mut m, plate, boss)| run_bool(&mut m, plate, boss, BooleanOp::Union),
            BatchSize::PerIteration,
        );
    });

    group.bench_function("union/chain3_prisms", |b| {
        b.iter_batched(
            || {
                let mut m = BRepModel::new();
                let base = box_at(&mut m, 30.0, 10.0, 10.0, 0.0, 0.0, 5.0);
                let p1 = box_at(&mut m, 10.0, 30.0, 10.0, -10.0, 5.0, 5.0);
                let p2 = box_at(&mut m, 10.0, 30.0, 10.0, 0.0, -5.0, 5.0);
                let p3 = box_at(&mut m, 10.0, 30.0, 10.0, 10.0, 5.0, 5.0);
                (m, base, p1, p2, p3)
            },
            |(mut m, base, p1, p2, p3)| {
                let u1 = run_bool(&mut m, base, p1, BooleanOp::Union);
                let u2 = run_bool(&mut m, u1, p2, BooleanOp::Union);
                run_bool(&mut m, u2, p3, BooleanOp::Union)
            },
            BatchSize::PerIteration,
        );
    });

    group.bench_function("difference/box_minus_cylinder_poke", |b| {
        b.iter_batched(
            || {
                let mut m = BRepModel::new();
                let plate = box_at(&mut m, 40.0, 40.0, 10.0, 0.0, 0.0, 5.0);
                let drill = cylinder(&mut m, Point3::new(6.0, 3.0, -3.0), Vector3::Z, 5.0, 16.0);
                (m, plate, drill)
            },
            |(mut m, plate, drill)| run_bool(&mut m, plate, drill, BooleanOp::Difference),
            BatchSize::PerIteration,
        );
    });

    group.bench_function("difference/straddling_bore_f7_offset10", |b| {
        b.iter_batched(
            || {
                let mut m = BRepModel::new();
                let plate = box_at(&mut m, 60.0, 60.0, 10.0, 0.0, 0.0, 5.0);
                let boss = cylinder(&mut m, Point3::new(0.0, 0.0, 0.0), Vector3::Z, 15.0, 20.0);
                let u = run_bool(&mut m, plate, boss, BooleanOp::Union);
                let bore = cylinder(&mut m, Point3::new(10.0, 0.0, -3.0), Vector3::Z, 8.0, 26.0);
                (m, u, bore)
            },
            |(mut m, u, bore)| run_bool(&mut m, u, bore, BooleanOp::Difference),
            BatchSize::PerIteration,
        );
    });

    group.bench_function("intersection/box_cylinder", |b| {
        b.iter_batched(
            || {
                let mut m = BRepModel::new();
                let plate = box_at(&mut m, 40.0, 40.0, 10.0, 0.0, 0.0, 5.0);
                let cyl = cylinder(&mut m, Point3::new(10.0, 0.0, -3.0), Vector3::Z, 12.0, 16.0);
                (m, plate, cyl)
            },
            |(mut m, plate, cyl)| run_bool(&mut m, plate, cyl, BooleanOp::Intersection),
            BatchSize::PerIteration,
        );
    });

    group.finish();
}

criterion_group!(benches, bench_boolean_classification);
criterion_main!(benches);
