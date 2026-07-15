//! Criterion baseline for the sketch constraint solver — SKETCH-DCM
//! campaign #45 Slice 2 (spec §2.9: "no sketch benchmark exists").
//!
//! Benches the generated dimensioned plate (see `tests/common/mod.rs`)
//! at 30 / 100 / 300 constraints, on both solver paths:
//!
//! - `dense_*` — the pre-Slice-2 one-big-system damped Newton
//!   (decomposition disabled via `set_decomposition_enabled(false)`);
//! - `decomposed_*` — connected-component decomposition (the shipped
//!   default).
//!
//! Solvers are built fresh per iteration (`iter_batched`, setup
//! untimed) because `solve` converges the instance in place — timing a
//! second solve of a converged system would measure the no-op exit.
//! Sample size is deliberately small (10): this is a scaling baseline,
//! not a micro-optimisation harness; baseline numbers are recorded in
//! `.superpowers/sdd/sketch-dcm-slice2-report.md`.

// Reason for `#![allow(clippy::expect_used)]` — bench-only file:
// failing loudly at the setup site is the desired failure mode; the
// workspace deny lints target production code.
#![allow(clippy::expect_used)]

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use geometry_engine::sketch2d::sketch_solver::{build_solver, SolveOptions};
use std::time::Duration;

#[path = "../tests/common/mod.rs"]
mod common;

use common::PlateSpec;

fn bench_sketch_solver(c: &mut Criterion) {
    let mut group = c.benchmark_group("sketch_solver");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(5));

    let sizes: [(&str, PlateSpec); 3] = [
        ("30", PlateSpec::SMALL),
        ("100", PlateSpec::MEDIUM),
        ("300", PlateSpec::LARGE),
    ];

    for (label, spec) in sizes {
        for (path, decomposed) in [("dense", false), ("decomposed", true)] {
            group.bench_function(format!("{path}_{label}"), |b| {
                b.iter_batched(
                    || {
                        let plate = common::generate_plate(&spec);
                        let mut solver = build_solver(&plate.sketch, SolveOptions::default())
                            .expect("default solve options are valid");
                        solver.set_decomposition_enabled(decomposed);
                        solver
                    },
                    |mut solver| solver.solve(),
                    BatchSize::PerIteration,
                );
            });
        }
    }

    group.finish();
}

criterion_group!(benches, bench_sketch_solver);
criterion_main!(benches);
