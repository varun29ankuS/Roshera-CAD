//! Criterion baseline for the assembly mate solver — kinematic-assembly
//! campaign Slice 3 (spec §3.8: "the missing scale evidence").
//!
//! Benches the 60-instance scale fixture (six seated fastened stacks of
//! nine on a plate + a perturbed six-bar revolute loop; the same
//! generator as `tests/solver_scale.rs`) on both solver paths:
//!
//! - `dense_60`      — the pre-Slice-3 one-big-system Gauss-Newton
//!   (`Assembly::solve`): a full SVD over 354 columns per iteration;
//! - `decomposed_60` — Slice-3 seated-fastened condensation + component
//!   split + DR-plan (`Assembly::solve_decomposed`): the six-bar loop
//!   cluster is the only numeric work left.
//!
//! Assemblies are cloned fresh per iteration (`iter_batched`, setup
//! untimed) because a solve converges the poses in place — timing a
//! second solve of a converged system would measure the no-op exit.
//! Sample size is deliberately small: this is a scaling baseline, not a
//! micro-optimisation harness (the sketch_solver.rs bench convention).

use assembly_engine::Assembly;
use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use std::time::Duration;

#[path = "../tests/common/mod.rs"]
mod scale;

fn bench_solver(c: &mut Criterion) {
    let mut group = c.benchmark_group("assembly_solver");
    group.sample_size(10);
    group.warm_up_time(Duration::from_millis(500));
    group.measurement_time(Duration::from_secs(10));

    let fixture = scale::scale_fixture();

    group.bench_function("dense_60", |b| {
        b.iter_batched(
            || fixture.clone(),
            |mut assembly: Assembly| assembly.solve(),
            BatchSize::PerIteration,
        )
    });
    group.bench_function("decomposed_60", |b| {
        b.iter_batched(
            || fixture.clone(),
            |mut assembly: Assembly| assembly.solve_decomposed(),
            BatchSize::PerIteration,
        )
    });

    group.finish();
}

criterion_group!(benches, bench_solver);
criterion_main!(benches);
