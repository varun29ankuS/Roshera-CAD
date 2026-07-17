//! Slice-3 scale gate (spec §3.8): the missing scale evidence — a
//! generated ~60-instance assembly (bolted-plate stacks + a six-bar
//! linkage loop; `tests/common/mod.rs`), decomposed vs dense, measured
//! in the SAME process so the comparison is load-fair.
//!
//! Pre-implementation there was NO solver benchmark and no decomposed
//! path: dense Gauss-Newton runs a full SVD over 6·(N−1) columns per
//! iteration — O((6N)³) — where the planner's work is a handful of
//! 6-column extends plus one ~30-column loop cluster.
//!
//! Wall-clock assertions are RELATIVE (decomposed < dense on the same
//! machine in the same run) plus one generous absolute budget derived
//! for gate co-load (the sketch slice-2 lesson: budgets must survive a
//! busy machine, 3000 ms there).

mod common;

use common::scale_fixture;
use std::time::{Duration, Instant};

#[test]
fn decomposed_beats_dense_on_the_sixty_instance_fixture() {
    let fixture = scale_fixture();

    let mut dense = fixture.clone();
    let dense_start = Instant::now();
    let dense_report = dense.solve();
    let dense_elapsed = dense_start.elapsed();

    let mut decomposed = fixture.clone();
    let dec_start = Instant::now();
    let (dec_report, stats) = decomposed.solve_decomposed();
    let dec_elapsed = dec_start.elapsed();

    assert!(dense_report.converged, "{dense_report:?}");
    assert!(dec_report.converged, "{dec_report:?}");
    eprintln!(
        "scale gate: dense {dense_elapsed:?} ({} iters) vs decomposed {dec_elapsed:?} \
         ({} iters) — stats {stats:?}",
        dense_report.iterations, dec_report.iterations
    );

    // The planner saw what it should have seen.
    assert_eq!(
        stats.condensation_merges, 54,
        "six seated stacks of nine condense: {stats:?}"
    );
    assert_eq!(stats.components, 1, "the six-bar is the only live work");
    assert_eq!(stats.loop_clusters, 1, "{stats:?}");
    assert_eq!(stats.fallbacks, 0, "{stats:?}");

    // Both paths end on the constraint manifold.
    for (label, solved) in [("dense", &dense), ("decomposed", &decomposed)] {
        for m in &solved.mates {
            assert!(
                solved.mate_violation(m) < 1e-7,
                "{label}: mate violated after solve"
            );
        }
    }

    // The scale gate: decomposed strictly beats whole-system dense on
    // the same machine in the same run (the margin is orders of
    // magnitude — a 354-column SVD per dense iteration vs one 30-column
    // cluster — so a relative assert is co-load-safe).
    assert!(
        dec_elapsed < dense_elapsed,
        "decomposed ({dec_elapsed:?}) must beat dense ({dense_elapsed:?})"
    );
    // Generous absolute budget for gate co-load (sketch slice-2 lesson).
    assert!(
        dec_elapsed < Duration::from_millis(3000),
        "decomposed took {dec_elapsed:?} (budget 3000 ms)"
    );
}

#[test]
fn scale_fixture_solve_is_deterministic() {
    let fixture = scale_fixture();
    let mut a = fixture.clone();
    let (ra, sa) = a.solve_decomposed();
    let mut b = fixture.clone();
    let (rb, sb) = b.solve_decomposed();
    assert_eq!(ra, rb);
    assert_eq!(sa, sb);
    for (x, y) in a.instances.iter().zip(b.instances.iter()) {
        assert_eq!(x.translation, y.translation, "byte-identical translations");
        assert_eq!(x.rotation, y.rotation, "byte-identical rotations");
    }
}
